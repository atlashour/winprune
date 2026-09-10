use super::model::Item;
use super::validate;
use thiserror::Error;
use toml::{Table, Value};

const EMBEDDED: &[(&str, &str)] = &[
    ("appx.toml", include_str!("../../catalog/appx.toml")),
    ("services.toml", include_str!("../../catalog/services.toml")),
    (
        "telemetry.toml",
        include_str!("../../catalog/telemetry.toml"),
    ),
    ("privacy.toml", include_str!("../../catalog/privacy.toml")),
    ("onedrive.toml", include_str!("../../catalog/onedrive.toml")),
    ("xbox.toml", include_str!("../../catalog/xbox.toml")),
    ("identity.toml", include_str!("../../catalog/identity.toml")),
    ("update.toml", include_str!("../../catalog/update.toml")),
    ("shell.toml", include_str!("../../catalog/shell.toml")),
];

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("{source_name}: {message}")]
    Parse {
        source_name: String,
        message: String,
    },
    #[error("catalog is invalid:\n{}", .0.join("\n"))]
    Invalid(Vec<String>),
    #[error("overlay item without an id")]
    OverlayWithoutId,
}

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub items: Vec<Item>,
}

impl Catalog {
    pub fn embedded() -> Result<Catalog, CatalogError> {
        let mut items = Vec::new();
        for (name, text) in EMBEDDED {
            items.extend(parse_items(name, text)?);
        }
        let catalog = Catalog { items };
        catalog.validate().map_err(CatalogError::Invalid)?;
        Ok(catalog)
    }

    pub fn parse(source_name: &str, text: &str) -> Result<Catalog, CatalogError> {
        let catalog = Catalog {
            items: parse_items(source_name, text)?,
        };
        catalog.validate().map_err(CatalogError::Invalid)?;
        Ok(catalog)
    }

    /// Overlay items match base items by `id`. Matching entries are merged key by key
    /// (overlay wins), so an overlay can flip `level` or `enabled` without repeating the
    /// whole item. Unknown ids are appended as new items and must be complete.
    pub fn with_overlay(self, source_name: &str, text: &str) -> Result<Catalog, CatalogError> {
        let overlay = parse_tables(source_name, text)?;
        let mut base: Vec<Table> = self
            .items
            .iter()
            .map(|item| to_table(item).expect("items round-trip through toml"))
            .collect();

        for patch in overlay {
            let id = patch
                .get("id")
                .and_then(Value::as_str)
                .ok_or(CatalogError::OverlayWithoutId)?
                .to_string();
            match base
                .iter_mut()
                .find(|t| t.get("id").and_then(Value::as_str) == Some(&id))
            {
                Some(existing) => {
                    for (key, value) in patch {
                        existing.insert(key, value);
                    }
                }
                None => base.push(patch),
            }
        }

        let mut items = Vec::with_capacity(base.len());
        for table in base {
            let item: Item =
                table
                    .try_into()
                    .map_err(|e: toml::de::Error| CatalogError::Parse {
                        source_name: source_name.to_string(),
                        message: e.message().to_string(),
                    })?;
            items.push(item);
        }
        let catalog = Catalog {
            items: items.into_iter().filter(|i| i.enabled).collect(),
        };
        catalog.validate().map_err(CatalogError::Invalid)?;
        Ok(catalog)
    }

    pub fn validate(&self) -> Result<(), Vec<String>> {
        validate::check(&self.items)
    }

    pub fn applicable(&self, build: u32) -> impl Iterator<Item = &Item> {
        self.items
            .iter()
            .filter(move |item| item.windows.matches(build))
    }

    pub fn get(&self, id: &str) -> Option<&Item> {
        self.items.iter().find(|item| item.id == id)
    }
}

fn parse_tables(source_name: &str, text: &str) -> Result<Vec<Table>, CatalogError> {
    let root: Table = text
        .parse()
        .map_err(|e: toml::de::Error| CatalogError::Parse {
            source_name: source_name.to_string(),
            message: e.message().to_string(),
        })?;
    let Some(items) = root.get("item") else {
        return Ok(Vec::new());
    };
    let Some(array) = items.as_array() else {
        return Err(CatalogError::Parse {
            source_name: source_name.to_string(),
            message: "'item' must be an array of tables".to_string(),
        });
    };
    array
        .iter()
        .map(|v| {
            v.as_table().cloned().ok_or_else(|| CatalogError::Parse {
                source_name: source_name.to_string(),
                message: "each [[item]] must be a table".to_string(),
            })
        })
        .collect()
}

fn parse_items(source_name: &str, text: &str) -> Result<Vec<Item>, CatalogError> {
    let mut items = Vec::new();
    for table in parse_tables(source_name, text)? {
        let item: Item = table
            .try_into()
            .map_err(|e: toml::de::Error| CatalogError::Parse {
                source_name: source_name.to_string(),
                message: e.message().to_string(),
            })?;
        if item.enabled {
            items.push(item);
        }
    }
    Ok(items)
}

fn to_table(item: &Item) -> Result<Table, toml::ser::Error> {
    Table::try_from(item)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Category, Level, Step};

    const MINIMAL: &str = r#"
[[item]]
id = "appx.demo"
name = "Demo app"
category = "appx"
level = "medium"
risk = "low"
summary = "Removes the demo app."

[[item.step]]
kind = "appx"
patterns = ["*Demo*"]
"#;

    #[test]
    fn parses_minimal_item() {
        let catalog = Catalog::parse("test", MINIMAL).unwrap();
        let item = catalog.get("appx.demo").unwrap();
        assert_eq!(item.category, Category::Appx);
        assert_eq!(item.level, Level::Medium);
        assert!(item.enabled);
        assert_eq!(
            item.step,
            vec![Step::Appx {
                patterns: vec!["*Demo*".to_string()]
            }]
        );
    }

    #[test]
    fn rejects_unknown_fields() {
        let text = MINIMAL.replace("summary =", "sumary =");
        assert!(Catalog::parse("test", &text).is_err());
    }

    #[test]
    fn overlay_overrides_level_by_id() {
        let overlay = r#"
[[item]]
id = "appx.demo"
level = "max"
warning = "Demo warning."
"#;
        let catalog = Catalog::parse("test", MINIMAL)
            .unwrap()
            .with_overlay("overlay", overlay)
            .unwrap();
        assert_eq!(catalog.get("appx.demo").unwrap().level, Level::Max);
        assert_eq!(catalog.get("appx.demo").unwrap().name, "Demo app");
    }

    #[test]
    fn overlay_disables_item_by_id() {
        let overlay = r#"
[[item]]
id = "appx.demo"
enabled = false
"#;
        let catalog = Catalog::parse("test", MINIMAL)
            .unwrap()
            .with_overlay("overlay", overlay)
            .unwrap();
        assert!(catalog.get("appx.demo").is_none());
    }

    #[test]
    fn overlay_adds_new_item() {
        let overlay = r#"
[[item]]
id = "services.demo"
name = "Demo service"
category = "services"
level = "high"
risk = "low"
summary = "Disables the demo service."
warning = "Demo warning."

[[item.step]]
kind = "service"
names = ["DemoSvc"]
startup = "disabled"
"#;
        let catalog = Catalog::parse("test", MINIMAL)
            .unwrap()
            .with_overlay("overlay", overlay)
            .unwrap();
        assert_eq!(catalog.items.len(), 2);
        assert!(catalog.get("services.demo").is_some());
    }

    #[test]
    fn overlay_without_id_is_an_error() {
        let overlay = r#"
[[item]]
level = "max"
"#;
        let err = Catalog::parse("test", MINIMAL)
            .unwrap()
            .with_overlay("overlay", overlay)
            .unwrap_err();
        assert!(matches!(err, CatalogError::OverlayWithoutId));
    }

    #[test]
    fn applicable_filters_by_build() {
        let text = format!(
            "{MINIMAL}\n[[item]]\nid = \"appx.new\"\nname = \"New\"\ncategory = \"appx\"\nlevel = \"medium\"\nrisk = \"low\"\nsummary = \"x\"\nwindows = {{ min_build = 26100 }}\n[[item.step]]\nkind = \"appx\"\npatterns = [\"*New*\"]\n"
        );
        let catalog = Catalog::parse("test", &text).unwrap();
        let old: Vec<_> = catalog.applicable(19045).map(|i| i.id.as_str()).collect();
        let new: Vec<_> = catalog.applicable(26100).map(|i| i.id.as_str()).collect();
        assert_eq!(old, vec!["appx.demo"]);
        assert_eq!(new, vec!["appx.demo", "appx.new"]);
    }

    #[test]
    fn embedded_catalog_validates() {
        let catalog = Catalog::embedded().unwrap();
        assert!(catalog.items.len() > 20);
    }
}
