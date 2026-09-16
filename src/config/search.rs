use anyhow::{bail, Result};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchField {
    App = 1,
    Title = 2,
    Exe = 4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchFields(u8);

impl SearchFields {
    pub(crate) fn contains(self, field: SearchField) -> bool {
        self.0 & field as u8 != 0
    }
}

impl FromStr for SearchFields {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        let mut fields = 0;
        for field in value.split(',').map(str::trim) {
            fields |= match field {
                "app" => SearchField::App as u8,
                "title" => SearchField::Title as u8,
                "exe" => SearchField::Exe as u8,
                _ => bail!("搜索字段须为 app/title/exe 的非空逗号分隔子集"),
            };
        }
        Ok(Self(fields))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_are_a_nonempty_valid_subset() {
        for value in ["app", "title", "exe", "exe, app", "title,app,exe"] {
            assert!(value.parse::<SearchFields>().is_ok());
        }
        for value in ["", "all", "app,", "app,name", "APP"] {
            assert!(value.parse::<SearchFields>().is_err());
        }
        let fields = "title,exe".parse::<SearchFields>().unwrap();
        assert!(!fields.contains(SearchField::App));
        assert!(fields.contains(SearchField::Title) && fields.contains(SearchField::Exe));
    }
}
