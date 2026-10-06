use super::*;
impl SearchEntry {
    pub(super) fn field(&self, kind: crate::config::SearchField) -> &str {
        match kind {
            crate::config::SearchField::App => &self.app,
            crate::config::SearchField::Title => &self.title,
            crate::config::SearchField::Exe => &self.executable,
        }
    }
    pub(super) fn display_text(&self) -> (crate::config::SearchField, String, String) {
        let title = crate::picker::label(&self.title);
        let (kind, primary) = if title.trim().is_empty() {
            (
                crate::config::SearchField::App,
                crate::picker::label(&self.app),
            )
        } else {
            (crate::config::SearchField::Title, title)
        };
        let secondary = std::path::Path::new(self.executable.as_ref())
            .file_stem()
            .map(|name| crate::picker::label(&name.to_string_lossy()))
            .unwrap_or_else(|| crate::picker::label(&self.app));
        (kind, primary, secondary)
    }
    pub(super) fn row(&self, text: Text) -> PickerRow {
        let (_, primary, secondary) = self.display_text();
        PickerRow {
            key: self.key.clone(),
            primary,
            secondary,
            highlights: self.highlights.clone(),
            meta: text
                .search_window_meta(self.elevated, self.minimized)
                .into(),
            icon: None,
            remembered: Default::default(),
        }
    }
}
