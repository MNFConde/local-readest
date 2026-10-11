//! Open Library work lookup for the works layer's auto-grouping: given a
//! title (+optional author) find the OL work key so different editions and
//! translations of the same work share a `work_id`. AI/self-made translations
//! won't match OL (expected — group those manually via the admin page and
//! link them with `source_hash` instead).

use serde_json::Value;

pub struct WorkHit {
    /// Open Library work key, normalized to its bare id (`OL…W`).
    pub work_id: String,
    /// Primary language of the matched edition, if listed.
    pub language: String,
}

/// Pick the first search doc that carries a work key. Pure function so the
/// mapping is unit-testable offline.
pub fn pick_work(docs: &[Value]) -> Option<WorkHit> {
    for d in docs {
        let key = d.get("work_key").and_then(Value::as_str).unwrap_or("");
        if key.is_empty() {
            continue;
        }
        let work_id = key.trim_start_matches("/works/").to_string();
        if work_id.is_empty() {
            continue;
        }
        return Some(WorkHit {
            work_id,
            language: d
                .get("language")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        });
    }
    None
}

/// Query Open Library for the work matching title+author.
pub async fn lookup_work(
    client: &reqwest::Client,
    title: &str,
    author: &str,
) -> anyhow::Result<Option<WorkHit>> {
    let q = if author.is_empty() {
        title.to_string()
    } else {
        format!("{title} {author}")
    };
    let url = format!(
        "https://openlibrary.org/search.json?q={}&limit=5",
        crate::proxy::urlencoded(&q)
    );
    let body: Value = client.get(&url).send().await?.json().await?;
    let docs = body.get("docs").and_then(Value::as_array).cloned().unwrap_or_default();
    Ok(pick_work(&docs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pick_work_takes_first_doc_with_work_key() {
        let docs = vec![
            json!({ "title": "no key here" }),
            json!({ "work_key": "/works/OL19540932W", "language": "eng" }),
            json!({ "work_key": "/works/OL999W", "language": "fra" }),
        ];
        let hit = pick_work(&docs).expect("second doc has a work key");
        assert_eq!(hit.work_id, "OL19540932W");
        assert_eq!(hit.language, "eng");
    }

    #[test]
    fn pick_work_none_without_keys() {
        let docs = vec![json!({ "title": "x" }), json!({ "work_key": "" })];
        assert!(pick_work(&docs).is_none());
        assert!(pick_work(&[]).is_none());
    }
}
