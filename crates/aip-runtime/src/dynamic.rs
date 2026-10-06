//! Runtime validation for `Json validated by <snapshot>`: answers are checked
//! against the question list stored in the pinned version of the schema row.
//! Question shape: { key, kind: TEXT|CHOICE|FILE, required, maxLength?, choices? }.

use serde_json::Value;

pub fn validate(answers: &Value, questions: &Value) -> Result<(), (String, String)> {
    let empty = serde_json::Map::new();
    let obj = match answers {
        Value::Object(m) => m,
        Value::Null => &empty,
        _ => return Err(("".into(), "answers must be an object".into())),
    };
    let qs = questions.as_array().cloned().unwrap_or_default();
    for k in obj.keys() {
        if !qs.iter().any(|q| q.get("key").and_then(|x| x.as_str()) == Some(k.as_str())) {
            return Err((k.clone(), format!("'{k}' is not a question of this form")));
        }
    }
    for q in &qs {
        let key = q.get("key").and_then(|x| x.as_str()).unwrap_or_default().to_string();
        let required = q.get("required").and_then(|x| x.as_bool()).unwrap_or(false);
        let v = obj.get(&key).filter(|v| !v.is_null() && v.as_str() != Some(""));
        let Some(v) = v else {
            if required {
                return Err((key.clone(), format!("'{key}' is required")));
            }
            continue;
        };
        match q.get("kind").and_then(|x| x.as_str()).unwrap_or("TEXT") {
            "CHOICE" => {
                let choices: Vec<&str> =
                    q.get("choices").and_then(|c| c.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
                let s = v.as_str().unwrap_or_default();
                if !choices.contains(&s) {
                    return Err((key.clone(), format!("'{key}' must be one of {}", choices.join(", "))));
                }
            }
            _ => {
                let s = v.as_str().ok_or_else(|| (key.clone(), format!("'{key}' must be text")))?;
                if let Some(max) = q.get("maxLength").and_then(|x| x.as_u64())
                    && s.chars().count() as u64 > max
                {
                    return Err((key.clone(), format!("'{key}' is longer than {max} characters")));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate;
    use serde_json::json;

    #[test]
    fn form_rules() {
        let qs = json!([{"key": "why", "kind": "TEXT", "required": true, "maxLength": 5}, {"key": "gender", "kind": "CHOICE", "required": false, "choices": ["M", "F"]}]);
        assert!(validate(&json!({"why": "hi"}), &qs).is_ok());
        assert_eq!(validate(&json!({}), &qs).expect_err("invalid").0, "why");
        assert_eq!(validate(&json!({"why": "toolong"}), &qs).expect_err("invalid").0, "why");
        assert_eq!(validate(&json!({"why": "ok", "gender": "X"}), &qs).expect_err("invalid").0, "gender");
        assert_eq!(validate(&json!({"why": "ok", "extra": 1}), &qs).expect_err("invalid").0, "extra");
    }
}
