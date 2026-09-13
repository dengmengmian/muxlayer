use serde_json::Value;

/// 递归剥除上游 JSON schema 里不被严格上游识别的字段：`strict`、
/// `additionalProperties`（任意值，DeepSeek 不论布尔还是对象值都拒识别）、
/// null-valued properties（并同步从 `required` 里摘掉）。同时递归进
/// `properties` / `items` / `anyOf` / `oneOf` / `allOf` / `$defs` / `definitions` 子树。
///
/// 只用于 provider quirk 需要的路径：Responses→Chat 由
/// `ProviderTransform::clean_schemas()` 门控（目前仅 DeepSeek），Anthropic→Chat
/// 由调用方的 `clean_for_deepseek` 参数门控。Anthropic 路径只需
/// [`prune_null_properties`]。
pub fn clean_schema_for_deepseek(value: &mut Value) {
    walk_schema(value, true);
}

/// 只删 null 值 property（并同步摘 `required`），保留 `additionalProperties` /
/// `strict` 等其它字段。null property 对任何上游（含官方 Anthropic）都是非法 schema。
pub fn prune_null_properties(value: &mut Value) {
    walk_schema(value, false);
}

fn walk_schema(value: &mut Value, deepseek: bool) {
    match value {
        Value::Object(map) => {
            if deepseek {
                map.remove("strict");
                // DeepSeek 不支持 additionalProperties，不论值是什么都删掉
                map.remove("additionalProperties");
            } else if let Some(ap) = map.get_mut("additionalProperties") {
                // 对象值 additionalProperties 是 map 的 value schema，里面也可能有 null property
                walk_schema(ap, deepseek);
            }

            // Clean null-valued properties，并把它们从 required 里摘掉
            let mut removed: Vec<String> = Vec::new();
            if let Some(Value::Object(props)) = map.get_mut("properties") {
                props.retain(|k, v| {
                    if v.is_null() {
                        removed.push(k.clone());
                        false
                    } else {
                        true
                    }
                });
                for (_, v) in props.iter_mut() {
                    walk_schema(v, deepseek);
                }
            }
            if !removed.is_empty() {
                if let Some(Value::Array(required)) = map.get_mut("required") {
                    required.retain(|r| {
                        r.as_str()
                            .is_none_or(|name| !removed.iter().any(|x| x == name))
                    });
                }
            }

            // Recurse into items
            if let Some(items) = map.get_mut("items") {
                walk_schema(items, deepseek);
            }

            // Recurse into anyOf/oneOf/allOf
            for key in &["anyOf", "oneOf", "allOf"] {
                if let Some(Value::Array(arr)) = map.get_mut(*key) {
                    for item in arr.iter_mut() {
                        walk_schema(item, deepseek);
                    }
                }
            }

            // Recurse into $defs/definitions
            for key in &["$defs", "definitions"] {
                if let Some(Value::Object(defs)) = map.get_mut(*key) {
                    for (_, v) in defs.iter_mut() {
                        walk_schema(v, deepseek);
                    }
                }
            }
        }
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                walk_schema(item, deepseek);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_removes_strict() {
        let mut schema = json!({"type": "object", "strict": true});
        clean_schema_for_deepseek(&mut schema);
        assert!(schema.get("strict").is_none());
        assert_eq!(schema["type"], "object");
    }

    #[test]
    fn test_removes_additional_properties_false() {
        let mut schema = json!({"type": "object", "additionalProperties": false});
        clean_schema_for_deepseek(&mut schema);
        assert!(schema.get("additionalProperties").is_none());
    }

    #[test]
    fn test_removes_additional_properties_true() {
        let mut schema = json!({"type": "object", "additionalProperties": true});
        clean_schema_for_deepseek(&mut schema);
        assert!(schema.get("additionalProperties").is_none());
    }

    #[test]
    fn test_removes_additional_properties_object() {
        // DeepSeek 拒识别 additionalProperties,不论值是布尔还是对象(原 quirk 结论,
        // 对象值保留无法对真实 DeepSeek 验证),一律删掉。
        let mut schema =
            json!({"type": "object", "additionalProperties": {"type": "string", "strict": true}});
        clean_schema_for_deepseek(&mut schema);
        assert!(
            schema.get("additionalProperties").is_none(),
            "schema={schema}"
        );
    }

    #[test]
    fn test_prunes_required_for_removed_null_properties() {
        let mut schema = json!({
            "type": "object",
            "properties": {"name": {"type": "string"}, "age": null},
            "required": ["name", "age"]
        });
        clean_schema_for_deepseek(&mut schema);
        assert_eq!(schema["required"], json!(["name"]));
    }

    #[test]
    fn test_removes_null_properties() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "age": null,
                "email": {"type": "string"}
            }
        });
        clean_schema_for_deepseek(&mut schema);
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("name"));
        assert!(!props.contains_key("age"));
        assert!(props.contains_key("email"));
    }

    #[test]
    fn test_recurses_into_items() {
        let mut schema = json!({
            "type": "array",
            "items": {"type": "object", "strict": true, "additionalProperties": false}
        });
        clean_schema_for_deepseek(&mut schema);
        let items = &schema["items"];
        assert!(items.get("strict").is_none());
        assert!(items.get("additionalProperties").is_none());
    }

    #[test]
    fn test_recurses_into_anyof_oneof_allof() {
        let mut schema = json!({
            "anyOf": [
                {"type": "object", "strict": true},
                {"type": "string"}
            ],
            "oneOf": [
                {"type": "number", "additionalProperties": false}
            ],
            "allOf": [
                {"type": "array", "strict": true}
            ]
        });
        clean_schema_for_deepseek(&mut schema);
        assert!(schema["anyOf"][0].get("strict").is_none());
        assert!(schema["oneOf"][0].get("additionalProperties").is_none());
        assert!(schema["allOf"][0].get("strict").is_none());
    }

    #[test]
    fn test_recurses_into_defs() {
        let mut schema = json!({
            "$defs": {
                "User": {"type": "object", "strict": true}
            },
            "definitions": {
                "Item": {"type": "object", "additionalProperties": false}
            }
        });
        clean_schema_for_deepseek(&mut schema);
        assert!(schema["$defs"]["User"].get("strict").is_none());
        assert!(schema["definitions"]["Item"]
            .get("additionalProperties")
            .is_none());
    }

    #[test]
    fn test_nested_complex_schema() {
        let mut schema = json!({
            "type": "object",
            "strict": true,
            "properties": {
                "users": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "strict": true,
                        "properties": {
                            "tags": {
                                "type": "array",
                                "items": {"type": "string", "strict": true}
                            }
                        }
                    }
                }
            }
        });
        clean_schema_for_deepseek(&mut schema);
        assert!(schema.get("strict").is_none());
        assert!(schema["properties"]["users"]["items"]
            .get("strict")
            .is_none());
        assert!(
            schema["properties"]["users"]["items"]["properties"]["tags"]["items"]
                .get("strict")
                .is_none()
        );
    }
}
