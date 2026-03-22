/*
 * apfs_raw_scan.c — Read APFS catalog B-tree directly from raw disk.
 * Proof of concept: enumerate all files with metadata by reading
 * the on-disk B-tree, bypassing the kernel VFS entirely.
 *
 * Requires: sudo (for /dev/rdisk access)
 * Usage: sudo ./apfs_raw_scan /dev/rdisk3
 *
 * APFS on-disk format reference:
 * https://developer.apple.com/support/downloads/Apple-File-System-Reference.pdf
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/stat.h>
#include <time.h>

/* APFS block size (always 4096 for internal SSDs) */
#define APFS_BLOCK_SIZE 4096

/* Object type masks */
#define OBJ_TYPE_MASK       0x0000ffff
#define OBJ_TYPE_CONTAINER_SUPERBLOCK 0x01
#define OBJ_TYPE_BTREE      0x02
#define OBJ_TYPE_BTREE_NODE 0x03
#define OBJ_TYPE_OMAP       0x0b
#define OBJ_TYPE_FS         0x0d

/* Object header flags */
#define OBJ_PHYSICAL        0x00000000
#define OBJ_EPHEMERAL       0x80000000
#define OBJ_VIRTUAL         0x00000000
#define OBJ_STORAGETYPE_MASK 0xc0000000

/* Magic numbers */
#define NXSB_MAGIC 0x4253584e  /* 'NXSB' */
#define APSB_MAGIC 0x42535041  /* 'APSB' */

/* Catalog record types */
#define DREC_VAL_TYPE  0x9   /* directory record */
#define INODE_VAL_TYPE 0x3   /* inode record */
#define DREC_EXT_TYPE  0x4   /* extended fields */

/* ── On-disk structures ─────────────────────────────────────── */

typedef struct {
    uint64_t cksum;       /* Fletcher-64 checksum */
    uint64_t oid;         /* object ID */
    uint64_t xid;         /* transaction ID */
    uint32_t type;        /* object type + flags */
    uint32_t subtype;
} __attribute__((packed)) obj_phys_t;

typedef struct {
    obj_phys_t header;
    uint32_t magic;       /* NXSB_MAGIC */
    uint32_t block_size;
    uint64_t block_count;
    uint64_t features;
    uint64_t ro_compat_features;
    uint64_t incompat_features;
    uint8_t  uuid[16];
    uint64_t next_oid;
    uint64_t next_xid;
    uint32_t xp_desc_blocks;
    uint32_t xp_data_blocks;
    uint64_t xp_desc_base;
    uint64_t xp_data_base;
    uint32_t xp_desc_next;
    uint32_t xp_data_next;
    uint32_t xp_desc_index;
    uint32_t xp_desc_len;
    uint32_t xp_data_index;
    uint32_t xp_data_len;
    uint64_t spaceman_oid;
    uint64_t omap_oid;
    uint64_t reaper_oid;
    uint32_t test_type;
    uint32_t max_file_systems;
    uint64_t fs_oid[];    /* variable-length array of volume OIDs */
} __attribute__((packed)) nx_superblock_t;

typedef struct {
    obj_phys_t header;
    uint32_t magic;       /* APSB_MAGIC */
    uint32_t fs_index;
    uint64_t features;
    uint64_t ro_compat_features;
    uint64_t incompat_features;
    uint64_t unmount_time;
    uint64_t reserve_block_count;
    uint64_t quota_block_count;
    uint64_t alloc_count;
    /* ... more fields ... */
    /* At offset 80 from start of struct (after header): */
    /* We need the omap_oid and root_tree_oid */
} __attribute__((packed)) apfs_superblock_partial_t;

/* B-tree node header */
typedef struct {
    obj_phys_t header;
    uint16_t flags;
    uint16_t level;       /* 0 = leaf */
    uint32_t nkeys;
    uint16_t table_space_offset;
    uint16_t table_space_length;
    uint16_t free_space_offset;
    uint16_t free_space_length;
    uint16_t key_free_list_offset;
    uint16_t key_free_list_length;
    uint16_t val_free_list_offset;
    uint16_t val_free_list_length;
} __attribute__((packed)) btree_node_phys_t;

#define BTNODE_LEAF    0x0002
#define BTNODE_FIXED   0x0004
#define BTNODE_ROOT    0x0001

/* Table of contents entry */
typedef struct {
    uint16_t key_offset;
    uint16_t key_length;
    uint16_t val_offset;
    uint16_t val_length;
} __attribute__((packed)) kvloc_t;

/* Fixed-size TOC entry (for omap) */
typedef struct {
    uint16_t key_offset;
    uint16_t val_offset;
} __attribute__((packed)) kvoff_t;

/* Object map key/value */
typedef struct {
    uint64_t oid;
    uint64_t xid;
} __attribute__((packed)) omap_key_t;

typedef struct {
    uint32_t flags;
    uint32_t size;
    uint64_t paddr;
} __attribute__((packed)) omap_val_t;

/* B-tree info (at end of root node) */
typedef struct {
    uint64_t fixed_kv_size;     /* fixed key+val sizes, or 0 */
    uint32_t longest_key;
    uint32_t longest_val;
    uint64_t key_count;
    uint64_t node_count;
} __attribute__((packed)) btree_info_t;

/* ── Globals ────────────────────────────────────────────────── */

static int disk_fd = -1;
static uint32_t block_size = APFS_BLOCK_SIZE;

static void *read_block(uint64_t bno) {
    void *buf = malloc(block_size);
    if (!buf) return NULL;
    if (pread(disk_fd, buf, block_size, bno * block_size) != block_size) {
        free(buf);
        return NULL;
    }
    return buf;
}

static void *read_blocks(uint64_t bno, uint32_t count) {
    size_t size = (size_t)count * block_size;
    void *buf = malloc(size);
    if (!buf) return NULL;
    if (pread(disk_fd, buf, size, bno * block_size) != (ssize_t)size) {
        free(buf);
        return NULL;
    }
    return buf;
}

int main(int argc, char **argv) {
    const char *dev = argc > 1 ? argv[1] : "/dev/rdisk3";

    printf("apfs_raw_scan: opening %s\n", dev);
    disk_fd = open(dev, O_RDONLY);
    if (disk_fd < 0) {
        perror("open");
        printf("Try: sudo %s %s\n", argv[0], dev);
        return 1;
    }

    /* Step 1: Read container superblock at block 0 */
    nx_superblock_t *nxsb = read_block(0);
    if (!nxsb || nxsb->magic != NXSB_MAGIC) {
        printf("ERROR: Not a valid APFS container (magic=%08x, expected %08x)\n",
            nxsb ? nxsb->magic : 0, NXSB_MAGIC);
        free(nxsb);
        close(disk_fd);
        return 1;
    }

    block_size = nxsb->block_size;
    printf("Container: block_size=%u, block_count=%llu\n", block_size, nxsb->block_count);
    printf("  omap_oid=%llu\n", nxsb->omap_oid);
    printf("  max_file_systems=%u\n", nxsb->max_file_systems);

    /* Read volume OIDs */
    /* The fs_oid array follows the fixed fields. We need to find the Data volume. */
    /* For now, just print what we find */
    uint32_t max_fs = nxsb->max_file_systems;
    if (max_fs > 10) max_fs = 10;

    printf("  Volume OIDs:");
    for (uint32_t i = 0; i < max_fs; i++) {
        if (nxsb->fs_oid[i] != 0)
            printf(" [%u]=%llu", i, nxsb->fs_oid[i]);
    }
    printf("\n\n");

    /* Step 2: Read the container's object map to resolve virtual OIDs to physical blocks */
    printf("Reading container object map at block %llu...\n", nxsb->omap_oid);
    obj_phys_t *omap_hdr = read_block(nxsb->omap_oid);
    if (!omap_hdr) {
        printf("ERROR: Failed to read omap\n");
        free(nxsb);
        close(disk_fd);
        return 1;
    }
    printf("  omap type=%08x\n", omap_hdr->type);

    /* The omap has a B-tree root OID at a known offset.
       omap_phys_t layout (after obj_phys_t header):
       uint32_t flags, uint32_t snap_count, uint32_t tree_type, uint32_t snap_tree_type,
       uint64_t tree_oid, ... */
    uint64_t omap_btree_oid = *(uint64_t *)((char *)omap_hdr + sizeof(obj_phys_t) + 16);
    printf("  omap B-tree root at block %llu\n", omap_btree_oid);

    /* Step 3: For each volume, resolve its virtual OID via the omap */
    /* We want the Data volume (disk3s5). It's likely fs_oid[4] based on diskutil listing */
    /* But let's try each one and look for the right volume */
    for (uint32_t vi = 0; vi < max_fs; vi++) {
        uint64_t vol_oid = nxsb->fs_oid[vi];
        if (vol_oid == 0) continue;

        printf("\nVolume %u: virtual OID %llu\n", vi, vol_oid);

        /* To resolve: scan omap B-tree for key matching this OID.
           For simplicity, read the omap B-tree root and scan keys.
           Real implementation would do proper B-tree traversal. */
        btree_node_phys_t *omap_root = read_block(omap_btree_oid);
        if (!omap_root) { printf("  failed to read omap root\n"); continue; }

        printf("  omap root: flags=%04x level=%u nkeys=%u\n",
            omap_root->flags, omap_root->level, omap_root->nkeys);

        if (omap_root->level == 0) {
            /* Leaf node — scan entries */
            char *data = (char *)omap_root;
            uint16_t toc_off = sizeof(btree_node_phys_t);
            char *keys_base = data + sizeof(btree_node_phys_t)
                + omap_root->table_space_offset + omap_root->table_space_length;
            /* Omap uses fixed-size keys (kvoff_t) */
            for (uint32_t i = 0; i < omap_root->nkeys; i++) {
                kvoff_t *toc = (kvoff_t *)(data + toc_off + i * sizeof(kvoff_t));
                omap_key_t *key = (omap_key_t *)(keys_base + toc->key_offset);
                /* Values grow from end of block */
                omap_val_t *val = (omap_val_t *)(data + block_size - toc->val_offset - sizeof(omap_val_t));

                if (key->oid == vol_oid) {
                    printf("  -> resolved to physical block %llu\n", val->paddr);

                    /* Read the volume superblock */
                    uint32_t *vol_data = read_block(val->paddr);
                    if (vol_data) {
                        uint32_t vol_magic = *(uint32_t *)((char *)vol_data + sizeof(obj_phys_t));
                        printf("  volume magic=%08x %s\n", vol_magic,
                            vol_magic == APSB_MAGIC ? "(APSB - valid)" : "(INVALID)");

                        if (vol_magic == APSB_MAGIC) {
                            /* Print volume name - it's at offset 0x270 in the volume superblock */
                            char *volname = (char *)vol_data + 0x270;
                            printf("  volume name: %.64s\n", volname);
                        }
                        free(vol_data);
                    }
                }
            }
        } else {
            printf("  omap root is non-leaf (level %u), need full B-tree traversal\n",
                omap_root->level);
        }
        free(omap_root);
    }

    printf("\nDone. This is a proof of concept.\n");
    printf("Full implementation would traverse the catalog B-tree to enumerate all files.\n");

    free(nxsb);
    close(disk_fd);
    return 0;
}
