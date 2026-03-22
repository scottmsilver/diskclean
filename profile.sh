#!/bin/bash
set -e

# Build with symbols (no strip) for profiling
echo "Building with symbols..."
CARGO_PROFILE_RELEASE_STRIP=none CARGO_PROFILE_RELEASE_DEBUG=2 \
  cargo build --release --target aarch64-apple-darwin --bench scan_bench 2>&1 | tail -2

BENCH=$(ls ./target/aarch64-apple-darwin/release/deps/scan_bench-* | grep -v '\.d$\|\.o$' | head -1)
echo "Binary: $BENCH"

# Codesign for profiling
codesign --force --sign - --entitlements /dev/stdin "$BENCH" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>com.apple.security.get-task-allow</key><true/>
</dict></plist>
PLIST

TRACE=/tmp/dc_profile.trace
rm -rf "$TRACE"

echo ""
echo "=== Profiling with xctrace (Time Profiler + I/O) ==="
sudo xctrace record \
  --template 'Time Profiler' \
  --output "$TRACE" \
  --no-prompt \
  --launch -- "$BENCH" 1

sudo chmod -R a+rX "$TRACE"

echo ""
echo "=== Extracting hotspots ==="

# Get load address
LOAD_ADDR=$(xctrace export --input "$TRACE" \
  --xpath '/trace-toc/run[@number="1"]/data/table[@schema="time-profile"]' 2>/dev/null | \
  grep -oE 'load-addr="0x[0-9a-f]+"' | grep -v 'dyld\|libsystem\|libc' | head -1 | \
  sed 's/load-addr="//;s/"//')

if [ -z "$LOAD_ADDR" ]; then
  # Fallback: search in full export
  LOAD_ADDR=$(xctrace export --input "$TRACE" \
    --xpath '/trace-toc/run[@number="1"]/data/table[@schema="time-profile"]' 2>/dev/null | \
    grep 'scan_bench' | grep -oE 'load-addr="0x[0-9a-f]+"' | head -1 | \
    sed 's/load-addr="//;s/"//')
fi
echo "Load address: $LOAD_ADDR"

# Export and symbolicate
EXPORT=/tmp/dc_profile_tp.xml
xctrace export --input "$TRACE" \
  --xpath '/trace-toc/run[@number="1"]/data/table[@schema="time-profile"]' \
  2>/dev/null > "$EXPORT"

echo ""
echo "=== System call hotspots ==="
grep -oE 'frame [^/]*name="[^"]*"' "$EXPORT" | \
  sed 's/.*name="//;s/"//' | \
  grep -v '^0x' | \
  sort | uniq -c | sort -rn | head -25

echo ""
echo "=== Our code hotspots (symbolicated) ==="
# Get top addresses from our binary
ADDRS=$(grep -oE 'frame [^/]*name="0x[0-9a-f]+"' "$EXPORT" | \
  sed 's/.*name="//;s/"//' | \
  sort | uniq -c | sort -rn | head -20)

echo "$ADDRS" | while read COUNT ADDR; do
  if [ -n "$LOAD_ADDR" ] && [ -n "$ADDR" ]; then
    SYM=$(atos -o "$BENCH" -l "$LOAD_ADDR" "$ADDR" 2>/dev/null || echo "(unknown)")
    printf "%4d  %s\n" "$COUNT" "$SYM"
  fi
done

echo ""
echo "=== I/O analysis (fs_usage style) ==="
echo "Run separately: sudo fs_usage -w -f filesys $BENCH_PID"
echo "Or: sudo iosnoop -p \$PID"
echo ""
echo "Trace saved: $TRACE"
echo "Open in Instruments.app for full analysis: open $TRACE"
