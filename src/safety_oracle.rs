//! LLM-based safety validation for cleanup actions.
//! Asks Gemini 2.5 Pro to assess whether a deletion is safe.

use std::process::Command;

#[derive(Debug, Clone)]
pub struct SafetyAssessment {
    pub safe: bool,
    pub confidence: f32,   // 0.0 to 1.0
    pub reasoning: String,
    pub warnings: Vec<String>,
}

/// Ask the LLM to assess whether deleting this path/category is safe.
/// Uses the Gemini API via curl (no SDK dependency).
pub fn assess_safety(
    category: &str,
    path: &str,
    size_bytes: u64,
    detail: &str,
    advice: &str,
) -> Result<SafetyAssessment, String> {
    let api_key = std::env::var("GEMINI_API_KEY")
        .or_else(|_| std::env::var("GOOGLE_API_KEY"))
        .map_err(|_| "Set GEMINI_API_KEY or GOOGLE_API_KEY environment variable".to_string())?;

    let size_human = bytesize::ByteSize(size_bytes).to_string();

    let prompt = format!(r#"You are a macOS filesystem safety expert. Evaluate whether the following cleanup action is safe.

CATEGORY: {category}
PATH: {path}
SIZE: {size_human}
DETAIL: {detail}
TOOL'S ADVICE: {advice}

Respond in EXACTLY this JSON format, nothing else:
{{
  "safe": true/false,
  "confidence": 0.0-1.0,
  "reasoning": "one sentence explanation",
  "warnings": ["warning1", "warning2"]
}}

Rules:
- "safe" means deleting this will NOT cause data loss, app breakage, or system instability
- Caches, build artifacts, package manager downloads, and temp files are generally safe
- User documents, photos, databases, and config files are NOT safe
- If the path is inside Library/Caches, .cache, node_modules, target/, DerivedData — almost always safe
- If path contains personal data (documents, photos, financial) — NOT safe even if backed up
- Cloud-synced files: safe ONLY if confirmed synced (the cloud has the copy)
- Conda/pip/npm caches: safe (re-downloads on next install)
- Virtual environments: safe if the project has requirements.txt/pyproject.toml
- IDE extensions: old versions are safe if current version exists
- Be conservative with confidence — 0.9+ only for obvious caches"#);

    let body = serde_json::json!({
        "contents": [{
            "parts": [{"text": prompt}]
        }],
        "generationConfig": {
            "temperature": 0.1,
            "maxOutputTokens": 256
        }
    });

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.1-pro:generateContent?key={}",
        api_key
    );

    let output = Command::new("curl")
        .args([
            "-s", "-X", "POST",
            &url,
            "-H", "Content-Type: application/json",
            "-d", &body.to_string(),
        ])
        .output()
        .map_err(|e| format!("curl failed: {}", e))?;

    if !output.status.success() {
        return Err(format!("API error: {}", String::from_utf8_lossy(&output.stderr)));
    }

    let response: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("JSON parse error: {}", e))?;

    // Extract the text from Gemini's response
    let text = response["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .ok_or_else(|| {
            format!("Unexpected response format: {}",
                serde_json::to_string_pretty(&response).unwrap_or_default())
        })?;

    // Parse the JSON from the LLM's response
    // Strip markdown code fences if present
    let json_str = text.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    let parsed: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| format!("Failed to parse LLM JSON: {} — raw: {}", e, text))?;

    Ok(SafetyAssessment {
        safe: parsed["safe"].as_bool().unwrap_or(false),
        confidence: parsed["confidence"].as_f64().unwrap_or(0.0) as f32,
        reasoning: parsed["reasoning"].as_str().unwrap_or("").to_string(),
        warnings: parsed["warnings"].as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default(),
    })
}

/// Batch-assess multiple items efficiently (one API call for up to 10 items).
pub fn batch_assess_safety(
    items: &[(String, String, u64, String, String)], // (category, path, size, detail, advice)
) -> Result<Vec<SafetyAssessment>, String> {
    let api_key = std::env::var("GEMINI_API_KEY")
        .or_else(|_| std::env::var("GOOGLE_API_KEY"))
        .map_err(|_| "Set GEMINI_API_KEY or GOOGLE_API_KEY".to_string())?;

    let mut item_descriptions = String::new();
    for (i, (cat, path, size, detail, _advice)) in items.iter().enumerate() {
        let size_human = bytesize::ByteSize(*size).to_string();
        item_descriptions.push_str(&format!(
            "\nITEM {}: category={}, path={}, size={}, detail={}\n",
            i + 1, cat, path, size_human, detail
        ));
    }

    let prompt = format!(r#"You are a macOS filesystem safety expert. Evaluate whether each cleanup action is safe.

{item_descriptions}

For EACH item, respond with a JSON array. Each element:
{{
  "item": 1,
  "safe": true/false,
  "confidence": 0.0-1.0,
  "reasoning": "one sentence",
  "warnings": []
}}

Rules:
- Caches, build artifacts, node_modules, target/, DerivedData = safe (0.95)
- Package manager caches (npm, pip, brew, cargo) = safe (0.95)
- Old IDE extension versions = safe (0.9)
- Virtual environments with requirements.txt = safe (0.85)
- User documents, photos, financial files = NOT safe (0.1)
- Cloud-synced: safe only if confirmed backed up (0.7)
- When in doubt, say not safe

Return ONLY the JSON array, no other text."#);

    let body = serde_json::json!({
        "contents": [{"parts": [{"text": prompt}]}],
        "generationConfig": { "temperature": 0.1, "maxOutputTokens": 1024 }
    });

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.1-pro:generateContent?key={}",
        api_key
    );

    let output = Command::new("curl")
        .args(["-s", "-X", "POST", &url, "-H", "Content-Type: application/json", "-d", &body.to_string()])
        .output()
        .map_err(|e| format!("curl failed: {}", e))?;

    let response: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("JSON parse: {}", e))?;

    let text = response["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .ok_or_else(|| "No text in response".to_string())?;

    let json_str = text.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    let parsed: Vec<serde_json::Value> = serde_json::from_str(json_str)
        .map_err(|e| format!("Parse array: {} — raw: {}", e, text))?;

    Ok(parsed.iter().map(|v| SafetyAssessment {
        safe: v["safe"].as_bool().unwrap_or(false),
        confidence: v["confidence"].as_f64().unwrap_or(0.0) as f32,
        reasoning: v["reasoning"].as_str().unwrap_or("").to_string(),
        warnings: v["warnings"].as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default(),
    }).collect())
}
