#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ViewportState {
    scroll_from_bottom: usize,
    total_rows: usize,
    page_rows: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowedRows {
    pub rows: Vec<String>,
    pub total_rows: usize,
    pub max_offset: usize,
    pub scroll_from_bottom: usize,
}

impl ViewportState {
    pub fn apply(&mut self, rows: Vec<String>, max_rows: usize) -> WindowedRows {
        let max_rows = max_rows.max(1);
        self.total_rows = rows.len();
        self.page_rows = max_rows;
        let max_offset = rows.len().saturating_sub(max_rows);
        self.scroll_from_bottom = self.scroll_from_bottom.min(max_offset);
        let end = rows.len().saturating_sub(self.scroll_from_bottom);
        let start = end.saturating_sub(max_rows);
        WindowedRows {
            rows: rows[start..end].to_vec(),
            total_rows: rows.len(),
            max_offset,
            scroll_from_bottom: self.scroll_from_bottom,
        }
    }

    pub fn scroll_up(&mut self, amount: usize) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_add(amount.max(1));
    }

    pub fn scroll_down(&mut self, amount: usize) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(amount.max(1));
    }

    pub fn page_up(&mut self) {
        self.scroll_up(self.page_rows.saturating_sub(1).max(1));
    }

    pub fn page_down(&mut self) {
        self.scroll_down(self.page_rows.saturating_sub(1).max(1));
    }

    pub fn jump_top(&mut self) {
        self.scroll_from_bottom = self.total_rows;
    }

    pub fn jump_bottom(&mut self) {
        self.scroll_from_bottom = 0;
    }

    pub fn at_tail(&self) -> bool {
        self.scroll_from_bottom == 0
    }

    pub fn status_text(&self) -> String {
        if self.at_tail() {
            "live tail".into()
        } else {
            format!("scroll +{} / {}", self.scroll_from_bottom, self.total_rows)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_scrolls_from_bottom() {
        let mut viewport = ViewportState::default();
        let rows = vec![
            "row1".to_string(),
            "row2".to_string(),
            "row3".to_string(),
            "row4".to_string(),
        ];
        let window = viewport.apply(rows.clone(), 2);
        assert_eq!(window.rows, vec!["row3".to_string(), "row4".to_string()]);

        viewport.scroll_up(1);
        let window = viewport.apply(rows, 2);
        assert_eq!(window.rows, vec!["row2".to_string(), "row3".to_string()]);
    }
}
