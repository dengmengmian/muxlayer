//! Line-level surgical edits to TOML files.
//!
//! Preserves user comments, blank lines, key style choices, and unrelated
//! keys/sections byte-for-byte. The cost is that we only support a few narrow
//! operations:
//!
//!   - [`upsert_top_level_key`] — replace or insert a top-level scalar key
//!     (top-level = appearing before any `[section]` header).
//!   - [`upsert_section`] — replace or insert a `[header]` section and its
//!     keys; the section runs from its header line to the next `[...]` /
//!     `[[...]]` header or EOF.
//!   - [`upsert_section_key`] — replace or insert one key inside a section.
//!   - [`remove_section`] / [`remove_top_level_key`] — 撤销上面两种写入
//!     (「切回官方配置」时只删 MuxLayer 写过的东西)。
//!   - [`section_body`] / [`top_level_raw_value`] — 只读取原文,用于把快照里
//!     用户原来的值原样写回。
//!
//! Why not use a real TOML parser like `toml_edit` for writing? Codex's
//! `config.toml` carries `[projects]` trust levels, `[mcp_servers]` configs,
//! `[notice]` migration tables, inline comments documenting non-obvious flags,
//! and `model_reasoning_effort` set on top by the user. Line-based editing
//! keeps every byte we don't own. (Read-only lookups use `toml_edit`.)
//!
//! 多行结构:逐行扫描时跟踪多行字符串(`"""` / `'''`)和未闭合的数组 / 内联表。
//! 这些结构的续行即使以 `[` 开头也不是 section header,也不会被当成 key 行。
//!
//! Limitations:
//!   - **Trailing line comments on replaced lines are dropped.** Acceptable
//!     trade-off since the affected keys are MuxLayer-owned.
//!   - **CRLF input → LF output.** `str::lines()` handles \r\n on the way in;
//!     we emit `\n`.

/// Insert or replace a top-level scalar key. `raw_value` is the TOML literal
/// — caller is responsible for quoting strings (`"\"OpenAI\""`) and using
/// the right bare form for numbers / bools.
///
/// Semantics:
///   - If `key` exists as a top-level key (before any `[section]`), the line
///     (plus any continuation lines of a multi-line value) is replaced in
///     place. Any comments / blank lines before it are kept.
///   - If `key` does not exist, the new line is inserted just before the
///     first section header; or appended to the end if the file has no
///     sections.
///   - Same-named keys *inside* sections are not touched.
pub fn upsert_top_level_key(content: &str, key: &str, raw_value: &str) -> String {
    let new_line = format!("{key} = {raw_value}");
    let mut out = String::new();
    let mut written = false;
    let mut hit_section = false;
    let mut skip_continuation = false;

    for (line, cont) in scan_lines(content) {
        if skip_continuation {
            if cont {
                continue;
            }
            skip_continuation = false;
        }
        // First section header: if we still haven't written, insert just before it.
        if !hit_section && !cont && is_section_header(line) {
            if !written {
                out.push_str(&new_line);
                out.push('\n');
                written = true;
            }
            hit_section = true;
            push_line(&mut out, line);
            continue;
        }
        // Top-level region, key-matching line → replace.
        if !hit_section && !written && !cont && top_level_key_name(line) == Some(key) {
            out.push_str(&new_line);
            out.push('\n');
            written = true;
            skip_continuation = true;
            continue;
        }
        push_line(&mut out, line);
    }

    // No section header and key not found → append.
    if !written {
        out.push_str(&new_line);
        out.push('\n');
    }

    preserve_final_newline(content, out)
}

/// Insert or replace a `[header]` section. `header` is the bare section path
/// (e.g. `model_providers.OpenAI`) — no surrounding brackets. `body` is the
/// section's key=value lines (one per line, no trailing newline required;
/// no `[header]` line — we add it).
///
/// Semantics:
///   - The first occurrence of `[header]` is replaced; the section's content
///     (until the next `[...]` / `[[...]]` header or EOF) is dropped and
///     swapped for `body`.
///   - Subsequent same-named occurrences (rare; usually malformed configs)
///     are removed.
///   - If `[header]` doesn't appear, the section is appended to the end of
///     the file with one blank line of separation from preceding content.
pub fn upsert_section(content: &str, header: &str, body: &str) -> String {
    let target = format!("[{header}]");
    let mut out = String::new();
    let mut inserted = false;
    let mut skipping = false;

    for (line, cont) in scan_lines(content) {
        // Inside the section we're replacing: drop lines until the next header.
        if skipping {
            if !cont && is_section_header(line) {
                skipping = false;
            } else {
                continue;
            }
        }

        if !cont && header_matches(line, &target) {
            if !inserted {
                out.push_str(&target);
                out.push('\n');
                out.push_str(body);
                if !body.ends_with('\n') {
                    out.push('\n');
                }
                inserted = true;
            }
            // skip the rest of this section even on duplicate occurrences
            skipping = true;
            continue;
        }
        push_line(&mut out, line);
    }

    if !inserted {
        // Append, with one blank line of separation if the file isn't empty.
        if !out.is_empty() && !out.ends_with("\n\n") {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
        }
        out.push_str(&target);
        out.push('\n');
        out.push_str(body);
        if !body.ends_with('\n') {
            out.push('\n');
        }
    }

    preserve_final_newline(content, out)
}

/// Insert or replace a scalar key **inside** `[header]`, leaving the rest of
/// that section byte-for-byte. If the section is missing, it is appended with
/// just this key.
pub fn upsert_section_key(content: &str, header: &str, key: &str, raw_value: &str) -> String {
    let target = format!("[{header}]");
    let new_line = format!("{key} = {raw_value}");
    let mut out = String::new();
    let mut in_target = false;
    let mut written = false;
    let mut saw_section = false;
    let mut skip_continuation = false;

    for (line, cont) in scan_lines(content) {
        if skip_continuation {
            if cont {
                continue;
            }
            skip_continuation = false;
        }
        if !cont && is_section_header(line) {
            if in_target && !written {
                out.push_str(&new_line);
                out.push('\n');
                written = true;
            }
            in_target = header_matches(line, &target);
            if in_target {
                saw_section = true;
            }
            push_line(&mut out, line);
            continue;
        }
        if in_target && !written && !cont && top_level_key_name(line) == Some(key) {
            out.push_str(&new_line);
            out.push('\n');
            written = true;
            skip_continuation = true;
            continue;
        }
        push_line(&mut out, line);
    }

    if in_target && !written {
        out.push_str(&new_line);
        out.push('\n');
    }

    if !saw_section {
        if !out.is_empty() && !out.ends_with("\n\n") {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
        }
        out.push_str(&target);
        out.push('\n');
        out.push_str(&new_line);
        out.push('\n');
    }

    preserve_final_newline(content, out)
}

/// 删除 `[header]` 段(header 行 + 段体,直到下一个 header 或 EOF;重复出现的
/// 同名段一并删除)。段不存在时原样返回。
pub fn remove_section(content: &str, header: &str) -> String {
    let target = format!("[{header}]");
    let mut out = String::new();
    let mut removed = false;
    let mut skipping = false;

    for (line, cont) in scan_lines(content) {
        if skipping {
            if !cont && is_section_header(line) {
                skipping = false;
            } else {
                continue;
            }
        }
        if !cont && header_matches(line, &target) {
            removed = true;
            skipping = true;
            continue;
        }
        push_line(&mut out, line);
    }

    if !removed {
        return content.to_string();
    }
    if out.is_empty() {
        return out;
    }
    preserve_final_newline(content, out)
}

/// 删除顶层 `key = ...` 行(连同多行值的续行)。section 内同名 key 不动;
/// 不存在时原样返回。
pub fn remove_top_level_key(content: &str, key: &str) -> String {
    let mut out = String::new();
    let mut removed = false;
    let mut hit_section = false;
    let mut skip_continuation = false;

    for (line, cont) in scan_lines(content) {
        if skip_continuation {
            if cont {
                continue;
            }
            skip_continuation = false;
        }
        if !cont && is_section_header(line) {
            hit_section = true;
        }
        if !hit_section && !cont && top_level_key_name(line) == Some(key) {
            removed = true;
            skip_continuation = true;
            continue;
        }
        push_line(&mut out, line);
    }

    if !removed {
        return content.to_string();
    }
    if out.is_empty() {
        return out;
    }
    preserve_final_newline(content, out)
}

/// `[header]` 首次出现时的段体原文(不含 header 行,每行以 `\n` 结尾)。
/// 段不存在返回 `None`;段存在但为空返回 `Some("")`。
pub fn section_body(content: &str, header: &str) -> Option<String> {
    let target = format!("[{header}]");
    let mut body: Option<String> = None;
    for (line, cont) in scan_lines(content) {
        match body.as_mut() {
            None => {
                if !cont && header_matches(line, &target) {
                    body = Some(String::new());
                }
            }
            Some(buf) => {
                if !cont && is_section_header(line) {
                    break;
                }
                push_line(buf, line);
            }
        }
    }
    body
}

/// 顶层 key 的值字面量原文(去掉行尾注释与首尾空白)。只支持单行值;
/// key 不存在、在 section 内、或值跨多行时返回 `None`。
pub fn top_level_raw_value(content: &str, key: &str) -> Option<String> {
    let lines = scan_lines(content);
    for (idx, (line, cont)) in lines.iter().enumerate() {
        if *cont {
            continue;
        }
        if is_section_header(line) {
            return None;
        }
        if top_level_key_name(line) != Some(key) {
            continue;
        }
        let next_is_continuation = lines.get(idx + 1).map(|(_, c)| *c).unwrap_or(false);
        if next_is_continuation {
            return None;
        }
        let eq = line.find('=')?;
        let value = strip_comment(&line[eq + 1..]).trim();
        return Some(value.to_string());
    }
    None
}

// ── helpers ─────────────────────────────────────────────────────

/// 逐行扫描时的词法状态:是否处在多行字符串或未闭合的数组 / 内联表之内。
#[derive(Default, Clone, Copy)]
struct LexState {
    ml_basic: bool,
    ml_literal: bool,
    depth: usize,
}

impl LexState {
    fn in_continuation(self) -> bool {
        self.ml_basic || self.ml_literal || self.depth > 0
    }
}

/// 切行并标注每行是否是上一行未闭合结构(多行字符串 / 数组 / 内联表)的续行。
/// 续行既不是 section header,也不是 key 行。
fn scan_lines(content: &str) -> Vec<(&str, bool)> {
    let mut state = LexState::default();
    let mut out = Vec::new();
    for line in content.lines() {
        let cont = state.in_continuation();
        if !cont && is_section_header(line) {
            // header 行的方括号不计入数组深度
            out.push((line, false));
            continue;
        }
        state = advance(state, line);
        out.push((line, cont));
    }
    out
}

/// 扫描一行,更新多行字符串 / 括号深度状态。注释(`#`)之后的内容忽略。
fn advance(mut st: LexState, line: &str) -> LexState {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if st.ml_basic {
            if b[i] == b'\\' {
                i += 2;
            } else if b[i..].starts_with(b"\"\"\"") {
                st.ml_basic = false;
                i += 3;
            } else {
                i += 1;
            }
            continue;
        }
        if st.ml_literal {
            if b[i..].starts_with(b"'''") {
                st.ml_literal = false;
                i += 3;
            } else {
                i += 1;
            }
            continue;
        }
        match b[i] {
            b'#' => break,
            b'"' if b[i..].starts_with(b"\"\"\"") => {
                st.ml_basic = true;
                i += 3;
            }
            b'"' => i = skip_basic_string(b, i + 1),
            b'\'' if b[i..].starts_with(b"'''") => {
                st.ml_literal = true;
                i += 3;
            }
            b'\'' => {
                i += 1;
                while i < b.len() && b[i] != b'\'' {
                    i += 1;
                }
                i += 1;
            }
            b'[' | b'{' => {
                st.depth += 1;
                i += 1;
            }
            b']' | b'}' => {
                st.depth = st.depth.saturating_sub(1);
                i += 1;
            }
            _ => i += 1,
        }
    }
    st
}

/// 从单行 basic string 的内容起点跳到闭合引号之后。
fn skip_basic_string(b: &[u8], mut i: usize) -> usize {
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    i
}

/// 去掉行尾注释(引号内的 `#` 不算注释)。
fn strip_comment(s: &str) -> &str {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'#' => return &s[..i],
            b'"' => i = skip_basic_string(b, i + 1),
            b'\'' => {
                i += 1;
                while i < b.len() && b[i] != b'\'' {
                    i += 1;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    s
}

fn push_line(out: &mut String, line: &str) {
    out.push_str(line);
    out.push('\n');
}

/// True if `line` begins a TOML section. Covers both `[section]` and
/// `[[array_of_tables]]`. 调用方负责先排除多行结构的续行。
fn is_section_header(line: &str) -> bool {
    line.trim_start().starts_with('[')
}

/// Does `line` name the section `target` (a string like
/// `"[model_providers.OpenAI]"`)? Tolerates surrounding whitespace and `#`
/// comments after the closing `]` (quoted `#` inside the header is kept).
fn header_matches(line: &str, target: &str) -> bool {
    strip_comment(line.trim_start()).trim_end() == target
}

/// Extract the bare key name of a `key = value` line. Returns `None` for
/// blanks, comments, section headers, and lines we don't recognise as a
/// simple bare-key assignment.
fn top_level_key_name(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('[') {
        return None;
    }
    let eq = trimmed.find('=')?;
    let key_part = trimmed[..eq].trim();
    if key_part.is_empty() {
        return None;
    }
    // TOML bare keys are `[A-Za-z0-9_-]+`. Quoted keys like `"a.b" = 1` we
    // don't support — none of our callers write or want them.
    if !key_part
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    Some(key_part)
}

/// If the input didn't end with a newline, neither does the output. This
/// keeps the diff against the user's file minimal — most editors preserve
/// the trailing-newline convention of the file they opened.
fn preserve_final_newline(input: &str, mut output: String) -> String {
    if input.is_empty() {
        return output;
    }
    let input_terminated = input.ends_with('\n');
    let output_terminated = output.ends_with('\n');
    if !input_terminated && output_terminated {
        output.pop();
    } else if input_terminated && !output_terminated {
        output.push('\n');
    }
    output
}

/// 只读查询:用 toml_edit 解析后取某个表路径下的 key(按 section 感知,不会把
/// `model_context_window` 误当成 `model`)。字符串返回其值,其它标量返回字面量。
/// 解析失败 / 不存在返回 `None`。
pub fn lookup_str(content: &str, path: &[&str]) -> Option<String> {
    let doc = content.parse::<toml_edit::DocumentMut>().ok()?;
    let (last, parents) = path.split_last()?;
    let mut item = doc.as_item();
    for seg in parents {
        item = item.get(seg)?;
    }
    let value = item.get(last)?;
    match value.as_str() {
        Some(s) => Some(s.to_string()),
        None => value.as_value().map(|v| v.to_string().trim().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── upsert_top_level_key ──

    #[test]
    fn top_level_inserts_into_empty_file() {
        let out = upsert_top_level_key("", "model_provider", "\"OpenAI\"");
        assert_eq!(out, "model_provider = \"OpenAI\"\n");
    }

    #[test]
    fn top_level_replaces_existing_key_in_place() {
        let input = "# my header comment\nmodel = \"gpt-5\"\nmodel_provider = \"old\"\nother = 1\n";
        let out = upsert_top_level_key(input, "model_provider", "\"OpenAI\"");
        assert_eq!(
            out,
            "# my header comment\nmodel = \"gpt-5\"\nmodel_provider = \"OpenAI\"\nother = 1\n"
        );
    }

    #[test]
    fn top_level_inserts_before_first_section() {
        let input = "[mcp_servers]\nfoo = 1\n";
        let out = upsert_top_level_key(input, "model_provider", "\"OpenAI\"");
        assert_eq!(out, "model_provider = \"OpenAI\"\n[mcp_servers]\nfoo = 1\n");
    }

    #[test]
    fn top_level_does_not_touch_key_inside_section() {
        let input = "[mcp_servers]\nmodel_provider = \"nope\"\n";
        let out = upsert_top_level_key(input, "model_provider", "\"OpenAI\"");
        // Inserted before [mcp_servers], the in-section key is untouched.
        assert_eq!(
            out,
            "model_provider = \"OpenAI\"\n[mcp_servers]\nmodel_provider = \"nope\"\n"
        );
    }

    #[test]
    fn top_level_appends_when_no_section_and_no_existing_key() {
        let input = "model = \"gpt-5\"\nother = 1\n";
        let out = upsert_top_level_key(input, "model_provider", "\"OpenAI\"");
        assert_eq!(
            out,
            "model = \"gpt-5\"\nother = 1\nmodel_provider = \"OpenAI\"\n"
        );
    }

    #[test]
    fn top_level_preserves_blank_lines_and_comments_above() {
        let input = "# Codex config\n\n# managed by me\napproval_policy = \"on-request\"\n";
        let out = upsert_top_level_key(input, "model_provider", "\"OpenAI\"");
        assert_eq!(
            out,
            "# Codex config\n\n# managed by me\napproval_policy = \"on-request\"\nmodel_provider = \"OpenAI\"\n"
        );
    }

    #[test]
    fn top_level_drops_trailing_line_comment_on_replaced_line_intentionally() {
        // 已知折衷：替换整行 → 丢掉行尾注释。AgentGate-managed key 才会被替换，
        // 用户也不该往这个 key 上加注释。
        let input = "model_provider = \"old\"  # was the old name\n";
        let out = upsert_top_level_key(input, "model_provider", "\"OpenAI\"");
        assert_eq!(out, "model_provider = \"OpenAI\"\n");
    }

    // ── upsert_section ──

    #[test]
    fn section_inserts_into_empty_file() {
        let out = upsert_section("", "model_providers.OpenAI", "name = \"OpenAI\"\n");
        assert_eq!(out, "[model_providers.OpenAI]\nname = \"OpenAI\"\n");
    }

    #[test]
    fn section_replaces_existing_body_keeps_header_position() {
        let input =
            "# pre\n[model_providers.OpenAI]\nold_key = 1\nstill_old = 2\n[mcp_servers]\nfoo = 1\n";
        let out = upsert_section(
            input,
            "model_providers.OpenAI",
            "name = \"OpenAI\"\nbase_url = \"http://x/v1\"\n",
        );
        assert_eq!(
            out,
            "# pre\n[model_providers.OpenAI]\nname = \"OpenAI\"\nbase_url = \"http://x/v1\"\n[mcp_servers]\nfoo = 1\n"
        );
    }

    #[test]
    fn section_replaces_at_end_of_file() {
        let input = "[model_providers.OpenAI]\nold = 1\nstill_old = 2\n";
        let out = upsert_section(input, "model_providers.OpenAI", "x = 1\n");
        assert_eq!(out, "[model_providers.OpenAI]\nx = 1\n");
    }

    #[test]
    fn section_appended_with_blank_line_when_missing() {
        let input = "model_provider = \"OpenAI\"\n";
        let out = upsert_section(input, "model_providers.OpenAI", "name = \"x\"\n");
        assert_eq!(
            out,
            "model_provider = \"OpenAI\"\n\n[model_providers.OpenAI]\nname = \"x\"\n"
        );
    }

    #[test]
    fn section_dedupes_duplicate_occurrences() {
        // pathological config with two copies of the same section
        let input = "[x]\na=1\n[x]\nb=2\n[y]\nc=3\n";
        let out = upsert_section(input, "x", "z=9\n");
        assert_eq!(out, "[x]\nz=9\n[y]\nc=3\n");
    }

    #[test]
    fn section_preserves_unrelated_sections() {
        let input =
            "[providers.deepseek]\nkey = \"sk-real\"\n[providers.kimi]\nkey = \"sk-kimi\"\n";
        let out = upsert_section(input, "providers.agentgate", "type = \"openai\"\n");
        assert_eq!(
            out,
            "[providers.deepseek]\nkey = \"sk-real\"\n[providers.kimi]\nkey = \"sk-kimi\"\n\n[providers.agentgate]\ntype = \"openai\"\n"
        );
    }

    #[test]
    fn section_does_not_match_when_name_is_only_a_prefix() {
        // `[model_providers.OpenAI]` should not be matched by header
        // `model_providers.Open` — header_matches checks full equality.
        let input = "[model_providers.OpenAI]\na = 1\n[model_providers.OpenAI2]\nb = 2\n";
        let out = upsert_section(input, "model_providers.Open", "c = 3\n");
        assert!(out.contains("[model_providers.OpenAI]"));
        assert!(out.contains("[model_providers.OpenAI2]"));
        assert!(out.contains("[model_providers.Open]"));
    }

    #[test]
    fn section_tolerates_trailing_comment_on_header_line() {
        let input = "[model_providers.OpenAI]  # AgentGate's hijack\nold = 1\n";
        let out = upsert_section(input, "model_providers.OpenAI", "new = 1\n");
        assert_eq!(out, "[model_providers.OpenAI]\nnew = 1\n");
    }

    #[test]
    fn section_array_of_tables_acts_as_boundary() {
        // `[[arr]]` is its own boundary — replacing `[x]` should stop at it.
        let input = "[x]\na = 1\nold = 2\n[[arr]]\nname = \"first\"\n";
        let out = upsert_section(input, "x", "a = 1\n");
        assert_eq!(out, "[x]\na = 1\n[[arr]]\nname = \"first\"\n");
    }

    // ── combo (mirrors the Codex apply path) ──

    #[test]
    fn combo_codex_apply_preserves_user_config() {
        let user_config = "\
# User's notes
approval_policy = \"on-request\"
model_reasoning_effort = \"high\"

[projects.\"/home/me/repo\"]
trust_level = \"trusted\"

[mcp_servers.local]
command = \"my-mcp-server\"
";
        let host = "127.0.0.1";
        let port = 9090;
        let token = "ag_local_xxx";

        // Mirror tools/codex.rs::apply: write model_provider + the [model_providers.OpenAI] section.
        let mut c = user_config.to_string();
        c = upsert_top_level_key(&c, "model_provider", "\"OpenAI\"");
        let body = format!(
            "name = \"OpenAI\"\nbase_url = \"http://{host}:{port}/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"{token}\"\nrequires_openai_auth = true\n"
        );
        c = upsert_section(&c, "model_providers.OpenAI", &body);

        // User's stuff survives.
        assert!(c.contains("approval_policy = \"on-request\""));
        assert!(c.contains("model_reasoning_effort = \"high\""));
        assert!(c.contains("[projects.\"/home/me/repo\"]"));
        assert!(c.contains("trust_level = \"trusted\""));
        assert!(c.contains("[mcp_servers.local]"));
        // Our managed bits land.
        assert!(c.contains("model_provider = \"OpenAI\""));
        assert!(c.contains("[model_providers.OpenAI]"));
        assert!(c.contains("base_url = \"http://127.0.0.1:9090/v1\""));
        assert!(c.contains("experimental_bearer_token = \"ag_local_xxx\""));
    }

    #[test]
    fn combo_codex_apply_idempotent() {
        // 第二次 apply 应该和第一次结果一致——line-level edit 必须幂等。
        let host = "127.0.0.1";
        let port = 9090;
        let token = "ag_local_xxx";
        let body = format!(
            "name = \"OpenAI\"\nbase_url = \"http://{host}:{port}/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"{token}\"\nrequires_openai_auth = true\n"
        );

        let mut once = String::new();
        once = upsert_top_level_key(&once, "model_provider", "\"OpenAI\"");
        once = upsert_section(&once, "model_providers.OpenAI", &body);

        let mut twice = once.clone();
        twice = upsert_top_level_key(&twice, "model_provider", "\"OpenAI\"");
        twice = upsert_section(&twice, "model_providers.OpenAI", &body);

        assert_eq!(once, twice, "second apply must be a no-op");
    }

    #[test]
    fn combo_atomcode_preserves_other_providers() {
        let user_config = "\
default_provider = \"deepseek\"

[providers.deepseek]
type = \"openai\"
api_key = \"sk-user-key\"
model = \"deepseek-chat\"
base_url = \"https://api.deepseek.com/v1\"

[providers.kimi]
type = \"openai\"
api_key = \"sk-kimi\"
";
        let mut c = user_config.to_string();
        c = upsert_top_level_key(&c, "default_provider", "\"agentgate\"");
        c = upsert_section(
            &c,
            "providers.agentgate",
            "type = \"openai\"\napi_key = \"ag_local_x\"\nmodel = \"agentgate\"\nbase_url = \"http://127.0.0.1:9090/v1\"\ncontext_window = 1000000\n",
        );

        assert!(c.contains("default_provider = \"agentgate\""));
        assert!(c.contains("[providers.agentgate]"));
        assert!(c.contains("[providers.deepseek]"));
        assert!(c.contains("api_key = \"sk-user-key\""));
        assert!(c.contains("[providers.kimi]"));
        assert!(c.contains("api_key = \"sk-kimi\""));
    }

    // ── final newline preservation ──

    #[test]
    fn preserves_no_trailing_newline() {
        let input = "model = \"gpt-5\""; // no trailing \n
        let out = upsert_top_level_key(input, "model_provider", "\"OpenAI\"");
        // 我们插入的是另一个顶级 key，原 key 之后 → 输出仍不以 \n 结尾。
        assert!(!out.ends_with('\n'));
        assert!(out.contains("model = \"gpt-5\""));
        assert!(out.contains("model_provider = \"OpenAI\""));
    }

    #[test]
    fn preserves_trailing_newline() {
        let input = "model = \"gpt-5\"\n";
        let out = upsert_top_level_key(input, "model_provider", "\"OpenAI\"");
        assert!(out.ends_with('\n'));
    }

    // ── 多行数组 / 多行字符串(item 15 回归) ──

    #[test]
    fn top_level_insert_ignores_multiline_array_continuation_starting_with_bracket() {
        // 续行以 `[` 开头的多行数组,旧实现把 `  ["a", "b"],` 当成 section header,
        // 把新 key 插进数组中间,Codex 配置直接解析失败。
        let input = "matrix = [\n  [\"a\", \"b\"],\n  [\"c\"],\n]\n\n[section]\nx = 1\n";
        let out = upsert_top_level_key(input, "model_provider", "\"OpenAI\"");
        let doc = out
            .parse::<toml_edit::DocumentMut>()
            .expect("must stay valid TOML");
        assert_eq!(doc["model_provider"].as_str(), Some("OpenAI"));
        assert_eq!(doc["section"]["x"].as_integer(), Some(1));
        assert!(out.starts_with("matrix = [\n  [\"a\", \"b\"],\n  [\"c\"],\n]\n"));
    }

    #[test]
    fn top_level_insert_ignores_bracket_lines_inside_multiline_string() {
        let input =
            "notes = \"\"\"\n[not a header]\n\"\"\"\nlit = '''\n[also not]\n'''\n[real]\ny = 2\n";
        let out = upsert_top_level_key(input, "model_provider", "\"OpenAI\"");
        let doc = out
            .parse::<toml_edit::DocumentMut>()
            .expect("must stay valid TOML");
        assert_eq!(doc["model_provider"].as_str(), Some("OpenAI"));
        assert_eq!(doc["notes"].as_str(), Some("[not a header]\n"));
        assert_eq!(doc["real"]["y"].as_integer(), Some(2));
    }

    #[test]
    fn section_replace_does_not_stop_at_multiline_array_continuation() {
        let input = "[x]\nargs = [\n  [\"--a\"],\n]\nold = 1\n[y]\nkeep = true\n";
        let out = upsert_section(input, "x", "new = 1\n");
        assert_eq!(out, "[x]\nnew = 1\n[y]\nkeep = true\n");
    }

    #[test]
    fn remove_section_drops_only_target_and_keeps_rest() {
        let input = "a = 1\n\n[mcp_servers.fs]\ncommand = \"npx\"\nargs = [\n  [\"x\"],\n]\n\n[mcp_servers.other]\ncommand = \"o\"\n";
        let out = remove_section(input, "mcp_servers.fs");
        assert_eq!(out, "a = 1\n\n[mcp_servers.other]\ncommand = \"o\"\n");
        assert_eq!(remove_section(input, "missing"), input);
    }

    #[test]
    fn remove_top_level_key_only_touches_top_level() {
        let input = "# c\nmodel_provider = \"OpenAI\"\nkeep = 1\n[s]\nmodel_provider = \"inner\"\n";
        let out = remove_top_level_key(input, "model_provider");
        assert_eq!(out, "# c\nkeep = 1\n[s]\nmodel_provider = \"inner\"\n");
    }

    #[test]
    fn section_body_returns_raw_lines_of_first_occurrence() {
        let input = "a = 1\n[model_providers.OpenAI]\nname = \"OpenAI\"\nbase_url = \"https://api.openai.com/v1\"\n\n[other]\nb = 2\n";
        assert_eq!(
            section_body(input, "model_providers.OpenAI").as_deref(),
            Some("name = \"OpenAI\"\nbase_url = \"https://api.openai.com/v1\"\n\n")
        );
        assert_eq!(section_body(input, "missing"), None);
    }

    #[test]
    fn top_level_raw_value_reads_literal_text() {
        let input =
            "model_context_window = 200000 # mine\nmodel_provider = \"openai\"\n[s]\nx = 1\n";
        assert_eq!(
            top_level_raw_value(input, "model_context_window").as_deref(),
            Some("200000")
        );
        assert_eq!(
            top_level_raw_value(input, "model_provider").as_deref(),
            Some("\"openai\"")
        );
        assert_eq!(top_level_raw_value(input, "x"), None);
    }

    #[test]
    fn section_key_replaces_inside_section_and_keeps_siblings() {
        let input = "[models]\ndefault = \"old\"\nweb_search = \"keep\"\n";
        let out = upsert_section_key(input, "models", "default", "\"muxlayer\"");
        assert_eq!(
            out,
            "[models]\ndefault = \"muxlayer\"\nweb_search = \"keep\"\n"
        );
    }

    #[test]
    fn section_key_appends_section_when_missing() {
        let out = upsert_section_key("", "models", "default", "\"muxlayer\"");
        assert_eq!(out, "[models]\ndefault = \"muxlayer\"\n");
    }

    #[test]
    fn section_key_inserts_into_existing_section_without_key() {
        let input = "[models]\nweb_search = \"keep\"\n";
        let out = upsert_section_key(input, "models", "default", "\"muxlayer\"");
        assert!(out.contains("web_search = \"keep\""));
        assert!(out.contains("default = \"muxlayer\""));
        assert!(out.contains("[models]"));
    }
}
