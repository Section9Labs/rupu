use super::*;
pub(super) struct Rendered {
    pub text: String,
    pub nodes: u32,
    pub marks: u32,
}
/// Bounded plain-text projection. Every unsupported node/mark stays explicit;
/// the result is never presented as the original ADF or Markdown bytes.
pub(super) fn render(value: &Value) -> Result<Rendered, ScmError> {
    if value["type"] != "doc" || value["version"] != 1 {
        return Err(failed());
    }
    let mut out = Rendered {
        text: String::new(),
        nodes: 0,
        marks: 0,
    };
    let mut seen = 0;
    walk(value, 0, &mut seen, &mut out)?;
    Ok(out)
}
fn append(out: &mut Rendered, s: &str) -> Result<(), ScmError> {
    if s.len() > MAX_PAGE_BYTES.saturating_sub(out.text.len()) {
        return Err(failed());
    }
    out.text.push_str(s);
    Ok(())
}
fn walk(v: &Value, depth: usize, seen: &mut usize, out: &mut Rendered) -> Result<(), ScmError> {
    *seen += 1;
    if depth > 64 || *seen > 10000 {
        return Err(failed());
    }
    let kind = v["type"].as_str().ok_or_else(failed)?;
    if let Some(marks) = v.get("marks") {
        let marks = marks.as_array().ok_or_else(failed)?;
        out.marks = out
            .marks
            .checked_add(u32::try_from(marks.len()).map_err(|_| failed())?)
            .ok_or_else(failed)?;
    }
    match kind {
        "text" => append(out, v["text"].as_str().ok_or_else(failed)?)?,
        "hardBreak" => append(out, "\n")?,
        "doc" | "paragraph" | "heading" | "blockquote" | "bulletList" | "orderedList"
        | "listItem" | "codeBlock" => {}
        _ => {
            out.nodes += 1;
            if let Some(s) = v["attrs"]["text"].as_str() {
                append(out, s)?;
            }
        }
    }
    if let Some(children) = v.get("content") {
        let children = children.as_array().ok_or_else(failed)?;
        for (index, child) in children.iter().enumerate() {
            if index > 0
                && matches!(
                    kind,
                    "doc" | "blockquote" | "bulletList" | "orderedList" | "listItem"
                )
            {
                append(out, "\n")?;
            }
            walk(child, depth + 1, seen, out)?;
        }
    }
    Ok(())
}
