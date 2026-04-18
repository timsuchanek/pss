use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::app::Bucket;
use crate::collector::Snapshot;

#[derive(Debug, Clone, Serialize)]
pub struct LlmDigest {
    pub total_cpu: f32,
    pub total_mem_gb: f32,
    pub buckets: Vec<DigestBucket>,
    pub top_procs: Vec<DigestProc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DigestBucket {
    pub label: String,
    pub cpu: f32,
    pub mem_mb: u64,
    pub proc_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct DigestProc {
    pub pid: u32,
    pub name: String,
    pub cpu: f32,
    pub mem_mb: u64,
    pub cwd: Option<String>,
}

impl LlmDigest {
    pub fn from_state(snap: &Snapshot, buckets: &[Bucket]) -> Self {
        let mut top = snap.procs.clone();
        top.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal));
        top.truncate(15);
        Self {
            total_cpu: snap.total_cpu,
            total_mem_gb: snap.total_mem as f32 / 1024.0 / 1024.0 / 1024.0,
            buckets: buckets
                .iter()
                .take(10)
                .map(|b| DigestBucket {
                    label: b.key.label(),
                    cpu: b.cpu,
                    mem_mb: b.mem / 1024 / 1024,
                    proc_count: b.pids.len(),
                })
                .collect(),
            top_procs: top
                .into_iter()
                .map(|p| DigestProc {
                    pid: p.pid,
                    name: p.name,
                    cpu: p.cpu,
                    mem_mb: p.mem / 1024 / 1024,
                    cwd: p.cwd.as_ref().map(|p| p.display().to_string()),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Recommendation {
    pub pid: Option<u32>,
    pub action: String,   // "kill", "reclaim", "throttle", "info"
    pub target: String,   // human-readable target
    pub reason: String,
    pub confidence: u8,   // 0..=100
    #[serde(default)]
    pub estimated_saved_mb: u64,
}

#[derive(Clone)]
pub struct LlmClient {
    inner: Arc<Inner>,
}

struct Inner {
    api_key: Option<String>,
    model: String,
    http: reqwest::Client,
}

impl LlmClient {
    pub fn new(api_key: Option<String>, model: String) -> Self {
        Self {
            inner: Arc::new(Inner {
                api_key,
                model,
                http: reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(20))
                    .build()
                    .unwrap(),
            }),
        }
    }

    pub async fn recommend(&self, digest: &LlmDigest) -> Result<Vec<Recommendation>> {
        let Some(key) = self.inner.api_key.as_ref() else {
            return Ok(Vec::new());
        };

        let system = "You are the advisor for a system monitor (pss). \
Given a JSON digest of processes grouped by cwd, return a JSON array of \
kill/reclaim recommendations. Respond with STRICT JSON only, no prose. \
Schema: [{pid: number|null, action: \"kill\"|\"reclaim\"|\"throttle\"|\"info\", \
target: string, reason: string (<= 100 chars), confidence: 0-100, \
estimated_saved_mb: number}]. Prefer high-confidence, genuinely reclaimable \
wins. Max 5 items. No markdown fences.";

        let body = serde_json::json!({
            "model": self.inner.model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user",   "content": serde_json::to_string(digest)? }
            ],
            "temperature": 0.2,
            "response_format": { "type": "json_object" }
        });

        let resp = self
            .inner
            .http
            .post("https://openrouter.ai/api/v1/chat/completions")
            .bearer_auth(key)
            .header("HTTP-Referer", "https://github.com/tim/pss")
            .header("X-Title", "pss")
            .json(&body)
            .send()
            .await
            .context("openrouter request")?;

        let val: serde_json::Value = resp.json().await.context("openrouter decode")?;
        let text = val["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| anyhow!("no content in response"))?;

        // Response may be an object wrapping the array, or the array itself.
        let parsed: serde_json::Value = serde_json::from_str(text).context("parse content")?;
        let arr = match parsed {
            serde_json::Value::Array(a) => a,
            serde_json::Value::Object(map) => map
                .into_iter()
                .find_map(|(_, v)| v.as_array().cloned())
                .unwrap_or_default(),
            _ => vec![],
        };

        let recs: Vec<Recommendation> = arr
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect();

        Ok(recs)
    }
}
