//! The CP's view of customers: row/detail DTOs, tints, the `?customer=`
//! filter, derived attribution for legacy runs, and per-customer pricing.
//!
//! Spec: `docs/superpowers/plans/2026-10-06-rupu-customers-plan-2a-cp-backend.md`
//! (rulings 4, 7, 8, 9).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

use rupu_codename::{crew_for, crew_tint};
use rupu_config::{LayerPaths, PricingConfig};
use rupu_workspace::{validate_slug, Customer, CustomerStore};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;

/// Neutral tint for a slug whose crew has no palette entry.
const NEUTRAL_LIGHT: &str = "#71717a";
const NEUTRAL_DARK: &str = "#a1a1aa";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TintDto {
    pub light: String,
    pub dark: String,
}

/// What a row shows about its customer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CustomerRef {
    pub slug: String,
    pub name: String,
    pub tint: TintDto,
    pub archived: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CustomerDto {
    pub slug: String,
    pub name: String,
    pub notes: Option<String>,
    pub contact: Option<String>,
    pub color: Option<String>,
    pub tint: TintDto,
    pub archived: bool,
    pub created_at: String,
}

/// A customer's tint: its explicit `color` for both themes when set, else the
/// crew tint derived from the slug (ruling 8), else a neutral grey.
pub fn tint_for(slug: &str, color: Option<&str>) -> TintDto {
    if let Some(c) = color {
        return TintDto {
            light: c.to_string(),
            dark: c.to_string(),
        };
    }
    match crew_tint(&crew_for(slug)) {
        Some(t) => TintDto {
            light: t.light.to_string(),
            dark: t.dark.to_string(),
        },
        None => TintDto {
            light: NEUTRAL_LIGHT.to_string(),
            dark: NEUTRAL_DARK.to_string(),
        },
    }
}

pub fn customer_ref(c: &Customer) -> CustomerRef {
    CustomerRef {
        slug: c.slug.clone(),
        name: c.meta.name.clone(),
        tint: tint_for(&c.slug, c.meta.color.as_deref()),
        archived: c.meta.archived,
    }
}

pub fn customer_dto(c: &Customer) -> CustomerDto {
    CustomerDto {
        slug: c.slug.clone(),
        name: c.meta.name.clone(),
        notes: c.meta.notes.clone(),
        contact: c.meta.contact.clone(),
        color: c.meta.color.clone(),
        tint: tint_for(&c.slug, c.meta.color.as_deref()),
        archived: c.meta.archived,
        created_at: c.meta.created_at.clone(),
    }
}

/// `?customer=<slug>` | `?customer=none`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomerFilter {
    Slug(String),
    /// Work with no customer (the picker's "Unassigned", ruling 7).
    Unassigned,
}

impl CustomerFilter {
    /// `None` = no filter. A malformed slug is a 400; `"none"` selects
    /// unassigned work.
    pub fn parse(raw: Option<&str>) -> Result<Option<Self>, ApiError> {
        let Some(raw) = raw else { return Ok(None) };
        if raw == "none" {
            return Ok(Some(Self::Unassigned));
        }
        validate_slug(raw).map_err(|e| ApiError::bad_request(format!("customer: {e}")))?;
        Ok(Some(Self::Slug(raw.to_string())))
    }

    pub fn matches(&self, customer: Option<&str>) -> bool {
        match self {
            Self::Slug(s) => customer == Some(s.as_str()),
            Self::Unassigned => customer.is_none(),
        }
    }
}

/// A run's customer as rows report it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribution {
    pub slug: Option<String>,
    /// True when `slug` came from the workspace's CURRENT assignment because
    /// the run recorded none (ruling 4).
    pub derived: bool,
}

/// Recorded customer, else the workspace's current assignment (`derived`),
/// else none. A workspace id the store rejects reads as "unknown", never an
/// error: an odd legacy id must not fail a listing.
pub fn attribute(store: &CustomerStore, recorded: Option<&str>, workspace_id: &str) -> Attribution {
    if let Some(slug) = recorded {
        return Attribution {
            slug: Some(slug.to_string()),
            derived: false,
        };
    }
    match store.customer_of(workspace_id) {
        Ok(Some(slug)) => Attribution {
            slug: Some(slug),
            derived: true,
        },
        Ok(None) => Attribution {
            slug: None,
            derived: false,
        },
        Err(e) => {
            tracing::debug!(workspace_id, error = %e, "customer attribution unavailable");
            Attribution {
                slug: None,
                derived: false,
            }
        }
    }
}

/// A per-request memo over a [`CustomerStore`]: each workspace's current
/// assignment, and each customer's record, is read at most once however many
/// rows share it (a legacy run's derived attribution otherwise costs one
/// sidecar read per row).
pub struct CustomerLookup {
    store: CustomerStore,
    assignments: HashMap<String, Option<String>>,
    refs: HashMap<String, CustomerRef>,
}

impl CustomerLookup {
    pub fn new(store: CustomerStore) -> Self {
        Self {
            store,
            assignments: HashMap::new(),
            refs: HashMap::new(),
        }
    }

    pub fn store(&self) -> &CustomerStore {
        &self.store
    }

    /// `ws_id`'s current assignment, as [`attribute`] reads it (a workspace
    /// id the store rejects reads as unassigned).
    pub fn assigned(&mut self, ws_id: &str) -> Option<String> {
        if let Some(hit) = self.assignments.get(ws_id) {
            return hit.clone();
        }
        let slug = attribute(&self.store, None, ws_id).slug;
        self.assignments.insert(ws_id.to_string(), slug.clone());
        slug
    }

    /// [`attribute`], memoized per workspace id.
    pub fn attribute(&mut self, recorded: Option<&str>, workspace_id: &str) -> Attribution {
        if let Some(slug) = recorded {
            return Attribution {
                slug: Some(slug.to_string()),
                derived: false,
            };
        }
        let slug = self.assigned(workspace_id);
        Attribution {
            derived: slug.is_some(),
            slug,
        }
    }

    /// The row reference for `slug`. A slug whose customer record is gone or
    /// unreadable (a dangling assignment) still shows, named by its slug —
    /// never as "no customer".
    pub fn customer_ref(&mut self, slug: &str) -> CustomerRef {
        if let Some(hit) = self.refs.get(slug) {
            return hit.clone();
        }
        let r = match self.store.get(slug) {
            Ok(c) => customer_ref(&c),
            Err(e) => {
                tracing::debug!(customer = slug, error = %e, "customer record unavailable");
                CustomerRef {
                    slug: slug.to_string(),
                    name: slug.to_string(),
                    tint: tint_for(slug, None),
                    archived: false,
                }
            }
        };
        self.refs.insert(slug.to_string(), r.clone());
        r
    }

    /// The customer a project is currently assigned to, as a row reference.
    pub fn project_customer(&mut self, ws_id: &str) -> Option<CustomerRef> {
        let slug = self.assigned(ws_id)?;
        Some(self.customer_ref(&slug))
    }
}

/// `(global config.toml mtime, customer config.toml mtime)`; `None` = absent.
type Stamps = (Option<SystemTime>, Option<SystemTime>);

fn mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Pricing for a customer's runs: global + that customer's layer, resolved
/// with `rupu_config::resolve` (locks honoured), cached per slug and
/// re-validated by the mtimes of the global and customer `config.toml`.
///
/// The no-customer baseline follows the same rule: the global file is
/// re-resolved (global layer only) when its mtime moves, so a global pricing
/// edit reaches customer and no-customer runs alike. The startup snapshot is
/// only the fallback for a global file that does not parse.
pub struct CustomerPricing {
    global_dir: PathBuf,
    startup: PricingConfig,
    store: CustomerStore,
    baseline: Mutex<Option<(Option<SystemTime>, PricingConfig)>>,
    cache: Mutex<HashMap<String, (Stamps, PricingConfig)>>,
}

impl CustomerPricing {
    pub fn new(global_dir: PathBuf, global: PricingConfig) -> Self {
        Self {
            store: CustomerStore::new(global_dir.clone()),
            global_dir,
            startup: global,
            baseline: Mutex::new(None),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// The global-only pricing, re-resolved when the global `config.toml`
    /// mtime changes; the startup snapshot if the file fails to parse.
    fn global_pricing(&self) -> PricingConfig {
        let global_path = self.global_dir.join("config.toml");
        let stamp = mtime(&global_path);
        let mut slot = self.baseline.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((cached, pricing)) = slot.as_ref() {
            if *cached == stamp {
                return pricing.clone();
            }
        }
        let pricing = match rupu_config::resolve(LayerPaths::global_only(&global_path)) {
            Ok(r) => r.config.pricing,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "global pricing unusable; using the startup snapshot"
                );
                self.startup.clone()
            }
        };
        // The fallback is cached too, so a bad file warns once per change.
        *slot = Some((stamp, pricing.clone()));
        pricing
    }

    /// `None` (or an invalid slug) => the global pricing. A customer whose
    /// layer is missing resolves to global pricing; one that is malformed
    /// warns once (until the file changes) and also falls back to global.
    pub fn for_customer(&self, slug: Option<&str>) -> PricingConfig {
        let Some(slug) = slug else {
            return self.global_pricing();
        };
        if let Err(e) = validate_slug(slug) {
            tracing::debug!(
                customer = slug,
                error = %e,
                "invalid customer slug; using global pricing"
            );
            return self.global_pricing();
        }
        let global_path = self.global_dir.join("config.toml");
        let customer_path = self.store.config_path(slug);
        let stamps: Stamps = (mtime(&global_path), mtime(&customer_path));

        {
            let cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
            if let Some((cached, pricing)) = cache.get(slug) {
                if *cached == stamps {
                    return pricing.clone();
                }
            }
        }
        let pricing = match rupu_config::resolve(LayerPaths::new(
            Some(&global_path),
            Some(&customer_path),
            None,
        )) {
            Ok(r) => r.config.pricing,
            Err(e) => {
                tracing::warn!(
                    customer = slug,
                    error = %e,
                    "customer pricing layer unusable; using global pricing"
                );
                self.global_pricing()
            }
        };
        // The fallback is cached too, so a bad layer warns once per change.
        self.cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(slug.to_string(), (stamps, pricing.clone()));
        pricing
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use rupu_config::pricing::lookup;
    use rupu_workspace::NewCustomer;
    use std::time::Duration;

    fn customer(slug: &str, color: Option<&str>) -> Customer {
        Customer {
            slug: slug.to_string(),
            meta: rupu_workspace::customers::CustomerMeta {
                name: "Acme Corp".into(),
                notes: Some("n".into()),
                contact: None,
                color: color.map(String::from),
                archived: true,
                created_at: "2026-10-06T00:00:00Z".into(),
            },
        }
    }

    #[test]
    fn tint_uses_explicit_color_for_both_themes() {
        let t = tint_for("acme", Some("#112233"));
        assert_eq!(t.light, "#112233");
        assert_eq!(t.dark, "#112233");
    }

    #[test]
    fn tint_derives_from_crew_and_is_stable() {
        let t = tint_for("acme", None);
        match crew_tint(&crew_for("acme")) {
            Some(c) => {
                assert_eq!(t.light, c.light);
                assert_eq!(t.dark, c.dark);
            }
            None => {
                assert_eq!(t.light, "#71717a");
                assert_eq!(t.dark, "#a1a1aa");
            }
        }
        assert_eq!(t, tint_for("acme", None));
    }

    #[test]
    fn ref_and_dto_carry_the_meta() {
        let c = customer("acme", Some("#abcdef"));
        let r = customer_ref(&c);
        assert_eq!(r.slug, "acme");
        assert_eq!(r.name, "Acme Corp");
        assert!(r.archived);
        assert_eq!(r.tint.light, "#abcdef");
        let d = customer_dto(&c);
        assert_eq!(d.notes.as_deref(), Some("n"));
        assert_eq!(d.color.as_deref(), Some("#abcdef"));
        assert_eq!(d.created_at, "2026-10-06T00:00:00Z");
    }

    #[test]
    fn filter_parses_and_matches() {
        assert_eq!(CustomerFilter::parse(None).unwrap(), None);
        let f = CustomerFilter::parse(Some("acme")).unwrap().unwrap();
        assert_eq!(f, CustomerFilter::Slug("acme".into()));
        let u = CustomerFilter::parse(Some("none")).unwrap().unwrap();
        assert_eq!(u, CustomerFilter::Unassigned);
        let err = CustomerFilter::parse(Some("Bad Slug")).unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);

        assert!(f.matches(Some("acme")));
        assert!(!f.matches(Some("other")));
        assert!(!f.matches(None));
        assert!(u.matches(None));
        assert!(!u.matches(Some("acme")));
    }

    fn assigned_store(home: &std::path::Path) -> CustomerStore {
        let store = CustomerStore::new(home);
        store
            .create(
                "acme",
                &NewCustomer {
                    name: "Acme".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        std::fs::create_dir_all(home.join("workspaces")).unwrap();
        std::fs::write(home.join("workspaces/ws_assigned.customer"), "acme\n").unwrap();
        store
    }

    #[test]
    fn attribution_recorded_derived_none() {
        let tmp = tempfile::tempdir().unwrap();
        let store = assigned_store(tmp.path());

        let a = attribute(&store, Some("zed"), "ws_assigned");
        assert_eq!((a.slug.as_deref(), a.derived), (Some("zed"), false));

        let a = attribute(&store, None, "ws_assigned");
        assert_eq!((a.slug.as_deref(), a.derived), (Some("acme"), true));

        let a = attribute(&store, None, "ws_unassigned");
        assert_eq!((a.slug, a.derived), (None, false));

        // A workspace id the store rejects never fails a listing.
        let a = attribute(&store, None, "../weird id");
        assert_eq!((a.slug, a.derived), (None, false));
    }

    #[test]
    fn lookup_memoizes_and_matches_attribute() {
        let tmp = tempfile::tempdir().unwrap();
        let store = assigned_store(tmp.path());
        let mut lookup = CustomerLookup::new(store.clone());

        for (recorded, ws) in [
            (Some("zed"), "ws_assigned"),
            (None, "ws_assigned"),
            (None, "ws_unassigned"),
            (None, "../weird id"),
        ] {
            assert_eq!(
                lookup.attribute(recorded, ws),
                attribute(&store, recorded, ws)
            );
        }

        // Memoized: removing the sidecar does not change this request's view.
        std::fs::remove_file(tmp.path().join("workspaces/ws_assigned.customer")).unwrap();
        assert_eq!(lookup.assigned("ws_assigned").as_deref(), Some("acme"));
        assert_eq!(CustomerLookup::new(store).assigned("ws_assigned"), None);

        let r = lookup.project_customer("ws_assigned").unwrap();
        assert_eq!((r.slug.as_str(), r.name.as_str()), ("acme", "Acme"));
        // A dangling slug still shows, named by its slug.
        let d = lookup.customer_ref("gone");
        assert_eq!((d.slug.as_str(), d.name.as_str()), ("gone", "gone"));
    }

    const GLOBAL_TOML: &str = "";

    fn write_layer(path: &std::path::Path, input: f64) {
        std::fs::write(
            path,
            format!(
                "[pricing.anthropic.\"claude-x\"]\ninput_per_mtok = {input}\noutput_per_mtok = 2.0\n"
            ),
        )
        .unwrap();
    }

    fn price(p: &PricingConfig) -> Option<f64> {
        lookup(p, "anthropic", "claude-x", "agent").map(|m| m.input_per_mtok)
    }

    #[test]
    fn customer_pricing_layers_and_invalidates_on_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::write(home.join("config.toml"), GLOBAL_TOML).unwrap();
        let store = CustomerStore::new(home);
        store
            .create(
                "acme",
                &NewCustomer {
                    name: "Acme".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let layer = store.config_path("acme");
        write_layer(&layer, 7.0);

        let pricing = CustomerPricing::new(home.to_path_buf(), PricingConfig::default());
        assert_eq!(price(&pricing.for_customer(Some("acme"))), Some(7.0));
        assert_eq!(price(&pricing.for_customer(None)), None);
        // Unknown customer (no layer file) ⇒ global.
        assert_eq!(price(&pricing.for_customer(Some("ghost"))), None);

        // Rewrite with a later mtime ⇒ the cache re-resolves.
        write_layer(&layer, 9.0);
        let later = SystemTime::now() + Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&layer)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert_eq!(price(&pricing.for_customer(Some("acme"))), Some(9.0));
    }

    fn set_mtime(path: &std::path::Path, t: SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    #[test]
    fn unchanged_stamps_serve_the_cached_value() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let store = CustomerStore::new(home);
        store
            .create(
                "acme",
                &NewCustomer {
                    name: "Acme".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let layer = store.config_path("acme");
        write_layer(&layer, 7.0);
        let old = std::fs::metadata(&layer).unwrap().modified().unwrap();

        let pricing = CustomerPricing::new(home.to_path_buf(), PricingConfig::default());
        assert_eq!(price(&pricing.for_customer(Some("acme"))), Some(7.0));

        // New contents under the OLD mtime: only the stamp says "unchanged",
        // so the cached 7.0 must still be served.
        write_layer(&layer, 9.0);
        set_mtime(&layer, old);
        assert_eq!(price(&pricing.for_customer(Some("acme"))), Some(7.0));
    }

    fn write_global(path: &std::path::Path, input: f64) {
        write_layer(path, input);
    }

    #[test]
    fn global_mtime_invalidates_a_customer_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let global = home.join("config.toml");
        // Global prices claude-x; the customer layer only prices claude-y.
        write_global(&global, 3.0);
        let store = CustomerStore::new(home);
        store
            .create(
                "acme",
                &NewCustomer {
                    name: "Acme".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        std::fs::write(
            store.config_path("acme"),
            "[pricing.anthropic.\"claude-y\"]\ninput_per_mtok = 5.0\noutput_per_mtok = 6.0\n",
        )
        .unwrap();

        let pricing = CustomerPricing::new(home.to_path_buf(), PricingConfig::default());
        assert_eq!(price(&pricing.for_customer(Some("acme"))), Some(3.0));

        write_global(&global, 4.0);
        set_mtime(&global, SystemTime::now() + Duration::from_secs(60));
        assert_eq!(price(&pricing.for_customer(Some("acme"))), Some(4.0));
    }

    #[test]
    fn no_customer_follows_global_edits() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let global = home.join("config.toml");
        write_global(&global, 3.0);
        let pricing = CustomerPricing::new(home.to_path_buf(), PricingConfig::default());
        assert_eq!(price(&pricing.for_customer(None)), Some(3.0));

        write_global(&global, 4.0);
        set_mtime(&global, SystemTime::now() + Duration::from_secs(60));
        assert_eq!(price(&pricing.for_customer(None)), Some(4.0));
    }

    #[test]
    fn unparsable_global_falls_back_to_the_startup_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::write(home.join("config.toml"), "= = nope").unwrap();
        let mut startup = PricingConfig::default();
        startup
            .models
            .entry("anthropic".into())
            .or_default()
            .insert(
                "claude-x".into(),
                rupu_config::ModelPricing {
                    input_per_mtok: 1.5,
                    output_per_mtok: 2.0,
                    cached_input_per_mtok: None,
                    cache_write_per_mtok: None,
                },
            );
        let pricing = CustomerPricing::new(home.to_path_buf(), startup);
        assert_eq!(price(&pricing.for_customer(None)), Some(1.5));
    }

    #[test]
    fn malformed_customer_layer_falls_back_to_global() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let store = CustomerStore::new(home);
        store
            .create(
                "acme",
                &NewCustomer {
                    name: "Acme".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        std::fs::write(store.config_path("acme"), "this is = = not toml").unwrap();
        write_layer(&home.join("config.toml"), 1.0);
        let pricing = CustomerPricing::new(home.to_path_buf(), PricingConfig::default());
        assert_eq!(price(&pricing.for_customer(Some("acme"))), Some(1.0));
    }
}
