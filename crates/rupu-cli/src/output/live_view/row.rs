use unicode_width::UnicodeWidthStr;

use crate::output::palette::Status;

/// Color role for a run of text. Plan 3's renderer maps each to ANSI;
/// `render_plain` ignores it (structure-only output for snapshots and
/// `--no-color`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Style {
    Plain,
    Dim,
    Strong,
    Status(Status),
    /// Run-level crew tint; carries the crew word for the renderer to look
    /// up via `rupu_codename::crew_tint`.
    Crew(String),
    /// Per-agent role hue; carries the role word for `rupu_codename::role_badge`.
    Role(String),
    Meter,
    Danger,
    Good,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    pub style: Style,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Line {
    pub segments: Vec<Segment>,
}

impl Line {
    pub fn new() -> Self {
        Self::default()
    }
    fn push(mut self, text: impl Into<String>, style: Style) -> Self {
        self.segments.push(Segment {
            text: text.into(),
            style,
        });
        self
    }
    pub fn plain(self, s: impl Into<String>) -> Self {
        self.push(s, Style::Plain)
    }
    pub fn dim(self, s: impl Into<String>) -> Self {
        self.push(s, Style::Dim)
    }
    pub fn strong(self, s: impl Into<String>) -> Self {
        self.push(s, Style::Strong)
    }
    pub fn status(self, st: Status, s: impl Into<String>) -> Self {
        self.push(s, Style::Status(st))
    }
    pub fn crew(self, crew: impl Into<String>, s: impl Into<String>) -> Self {
        self.push(s, Style::Crew(crew.into()))
    }
    pub fn role(self, role: impl Into<String>, s: impl Into<String>) -> Self {
        self.push(s, Style::Role(role.into()))
    }
    pub fn meter(self, s: impl Into<String>) -> Self {
        self.push(s, Style::Meter)
    }
    pub fn danger(self, s: impl Into<String>) -> Self {
        self.push(s, Style::Danger)
    }
    pub fn good(self, s: impl Into<String>) -> Self {
        self.push(s, Style::Good)
    }

    pub fn width(&self) -> usize {
        self.segments.iter().map(|seg| seg.text.width()).sum()
    }
}

pub fn render_plain(lines: &[Line]) -> String {
    lines
        .iter()
        .map(|l| {
            let mut s = String::new();
            for seg in &l.segments {
                s.push_str(&seg.text);
            }
            s.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Clip `line` to `max_cols` display columns, ending with `…` when clipped.
pub fn truncate_to(line: Line, max_cols: usize) -> Line {
    if line.width() <= max_cols {
        return line;
    }
    let budget = max_cols.saturating_sub(1); // room for the ellipsis
    let mut out = Line::new();
    let mut used = 0usize;
    for seg in line.segments {
        let w = seg.text.width();
        if used + w <= budget {
            used += w;
            out.segments.push(seg);
        } else {
            let mut kept = String::new();
            for ch in seg.text.chars() {
                let cw = ch.to_string().width();
                if used + cw > budget {
                    break;
                }
                used += cw;
                kept.push(ch);
            }
            if !kept.is_empty() {
                out.segments.push(Segment {
                    text: kept,
                    style: seg.style,
                });
            }
            break;
        }
    }
    out.segments.push(Segment {
        text: "…".to_string(),
        style: Style::Dim,
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::palette::Status;

    #[test]
    fn render_plain_joins_segments_and_trims_trailing() {
        let lines = vec![
            Line::new()
                .status(Status::Complete, "✓ ")
                .strong("preflight")
                .plain("   "),
            Line::new().dim("│"),
        ];
        assert_eq!(render_plain(&lines), "✓ preflight\n│");
    }

    #[test]
    fn truncate_to_clips_with_ellipsis() {
        let line = Line::new().plain("abcdefghij");
        assert_eq!(render_plain(&[truncate_to(line, 5)]), "abcd…");
    }

    #[test]
    fn width_counts_display_columns() {
        assert_eq!(Line::new().plain("ab").dim("cd").width(), 4);
    }
}
