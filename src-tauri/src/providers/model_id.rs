//! 模型 id 的通用处理，provider 能力矩阵、转换层、reasoning_store 共用。

/// 去掉模型 id 末尾的 `[...]` 限定后缀，如 `mimo-v2.5-pro[1m]` → `mimo-v2.5-pro`。
/// 能力矩阵、provider 模型白名单都按 base id 存。不做 trim / 大小写处理，
/// 需要时由调用方先行处理。
pub fn strip_qualifier(model: &str) -> &str {
    if let Some(stripped) = model.strip_suffix(']') {
        if let Some(open) = stripped.rfind('[') {
            return &stripped[..open];
        }
    }
    model
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_trailing_bracket_qualifier_only() {
        assert_eq!(strip_qualifier("mimo-v2.5-pro[1m]"), "mimo-v2.5-pro");
        assert_eq!(strip_qualifier("deepseek-v4-pro"), "deepseek-v4-pro");
        assert_eq!(strip_qualifier("a[x]b"), "a[x]b");
        assert_eq!(strip_qualifier("broken]"), "broken]");
    }
}
