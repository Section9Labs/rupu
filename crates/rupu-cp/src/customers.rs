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
use rupu_config::{KeySource, LayerPaths, LockOwner, PricingConfig};
use rupu_workspace::{validate_slug, Customer, CustomerError, CustomerStore};
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

pub use rupu_transcript::{Recorded, RecordedField};

/// `UsageSummary.pricing_error` for work whose customer can't be known (its
/// workspace's assignment is unreadable; an unfiltered list degrades): it is
/// priced at the global rates, which may be wrong.
pub const UNKNOWN_CUSTOMER_PRICING: &str = "customer unknown; priced at global rates";

/// A run's customer as rows report it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribution {
    pub slug: Option<String>,
    /// True when `slug` did not come from the work's own record: the
    /// workspace's CURRENT assignment (a legacy record, ruling 4) or a
    /// session's customer inherited by a legacy session turn.
    pub derived: bool,
}

impl Attribution {
    /// Mark an attribution whose record was inherited from a session
    /// ([`session_turn_customer`]) as derived: a session-inherited customer
    /// is never the turn's own record.
    pub fn inherited(mut self, inherited: bool) -> Self {
        if inherited && self.slug.is_some() {
            self.derived = true;
        }
        self
    }

    /// What a record says on its own, with no store to consult: a recorded
    /// slug or a recorded none; `None` for a legacy record (whose customer
    /// only the workspace's current assignment could tell).
    pub fn recorded_only(recorded: Recorded<'_>) -> Option<Self> {
        match recorded {
            Recorded::Legacy => None,
            Recorded::None => Some(Self {
                slug: None,
                derived: false,
            }),
            Recorded::Slug(s) => Some(Self {
                slug: Some(s.to_string()),
                derived: false,
            }),
        }
    }
}

/// A session turn's customer field: its own transcript's when that recorded
/// one (a slug or `null`); for a LEGACY turn transcript, the session
/// record's (its latest turn's — `true` = inherited, which rows report as
/// `customer_derived`); legacy when both are legacy (then the workspace's
/// current assignment applies).
pub fn session_turn_customer(
    head: &RecordedField,
    session: &RecordedField,
) -> (RecordedField, bool) {
    match (head, session) {
        (Some(_), _) => (head.clone(), false),
        (None, Some(_)) => (session.clone(), true),
        (None, None) => (None, false),
    }
}

/// The customer a record recorded (a slug, or `null` = none — neither
/// derived); for a LEGACY record (no key: it predates customers) only, the
/// workspace's current assignment (`derived`), else none. A recorded none
/// stays none even if the project has been assigned since: reassigning a
/// project never rewrites history. A workspace id the store rejects (a
/// malformed legacy id) reads as unassigned: nothing can be assigned under
/// it. Any other failure to read the assignment — an unreadable sidecar,
/// say — is a 500 naming the workspace: callers fail the request rather than
/// count the work as having no customer.
pub fn attribute(
    store: &CustomerStore,
    recorded: Recorded<'_>,
    workspace_id: &str,
) -> Result<Attribution, ApiError> {
    if let Some(who) = Attribution::recorded_only(recorded) {
        return Ok(who);
    }
    let slug = match store.customer_of(workspace_id) {
        Ok(slug) => slug,
        Err(CustomerError::InvalidWsId(_)) => None,
        Err(e) => return Err(assignment_error(workspace_id, &e)),
    };
    Ok(Attribution {
        derived: slug.is_some(),
        slug,
    })
}

/// The 500 for an assignment that cannot be read, naming the workspace.
fn assignment_error(ws_id: &str, e: &CustomerError) -> ApiError {
    ApiError::internal(format!(
        "the customer assignment of workspace {ws_id} cannot be read: {e}; \
         repair or remove `workspaces/{ws_id}.customer` under the rupu home"
    ))
}

/// A list row's customer keys, `#[serde(flatten)]`ed onto the row:
/// `customer` (`null` = no customer) and `customer_derived`. A row whose
/// customer cannot be known holds `None` in place of this and carries
/// NEITHER key — a coordinator then reads that host as unable to report a
/// customer for every run, never as "no customer".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RowCustomer {
    pub customer: Option<String>,
    pub customer_derived: bool,
}

impl From<Attribution> for RowCustomer {
    fn from(who: Attribution) -> Self {
        Self {
            customer: who.slug,
            customer_derived: who.derived,
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
    /// Workspaces [`Self::attribute_for_listing`] already warned about.
    warned: std::collections::HashSet<String>,
}

impl CustomerLookup {
    pub fn new(store: CustomerStore) -> Self {
        Self {
            store,
            assignments: HashMap::new(),
            refs: HashMap::new(),
            warned: std::collections::HashSet::new(),
        }
    }

    pub fn store(&self) -> &CustomerStore {
        &self.store
    }

    /// `ws_id`'s current assignment, as [`attribute`] reads it. An
    /// assignment that cannot be read fails the request (500 naming the
    /// workspace).
    pub fn assigned(&mut self, ws_id: &str) -> Result<Option<String>, ApiError> {
        if let Some(hit) = self.assignments.get(ws_id) {
            return Ok(hit.clone());
        }
        let slug = attribute(&self.store, Recorded::Legacy, ws_id)?.slug;
        self.assignments.insert(ws_id.to_string(), slug.clone());
        Ok(slug)
    }

    /// [`attribute`], memoized per workspace id.
    pub fn attribute(
        &mut self,
        recorded: Recorded<'_>,
        workspace_id: &str,
    ) -> Result<Attribution, ApiError> {
        if let Some(who) = Attribution::recorded_only(recorded) {
            return Ok(who);
        }
        let slug = self.assigned(workspace_id)?;
        Ok(Attribution {
            derived: slug.is_some(),
            slug,
        })
    }

    /// [`Self::attribute`] for a display listing — a CLI listing (`rupu run
    /// list`, `transcript list`, `session list`) or one of the CP's
    /// UNFILTERED run / agent-run / session lists — which must not fail on
    /// one workspace's unreadable assignment: `None` = the row's customer
    /// cannot be known (the row then omits its customer keys), with ONE
    /// warning per affected workspace naming it. A customer filter, and every
    /// count, rollup and price (customers API, project rollups, usage,
    /// dashboard), uses [`Self::attribute`] and fails closed instead.
    pub fn attribute_for_listing(
        &mut self,
        recorded: Recorded<'_>,
        workspace_id: &str,
    ) -> Option<Attribution> {
        match self.attribute(recorded, workspace_id) {
            Ok(who) => Some(who),
            Err(e) => {
                if self.warned.insert(workspace_id.to_string()) {
                    tracing::warn!(
                        workspace = workspace_id,
                        "{}; its rows are listed without a customer",
                        e.1
                    );
                }
                None
            }
        }
    }

    /// A workflow run's attribution. A MIRRORED worker run (`worker_id`
    /// set: a tunnel / bucket / placed unit's run held in this coordinator's
    /// store) is never attributed through THIS coordinator's assignments —
    /// its workspace lives on the worker, whose own customer namespace
    /// resolved it: only what it recorded counts ([`Attribution::recorded_only`];
    /// `None` for a legacy one, whose customer this host cannot know). A
    /// local run: [`Self::attribute`] when `fail_closed` (filters, counts,
    /// rollups, prices), else [`Self::attribute_for_listing`] (`None` = its
    /// assignment can't be read).
    pub fn attribute_run(
        &mut self,
        r: &rupu_orchestrator::RunRecord,
        fail_closed: bool,
    ) -> Result<Option<Attribution>, ApiError> {
        let recorded = Recorded::of(&r.customer);
        if r.worker_id.is_some() {
            return Ok(Attribution::recorded_only(recorded));
        }
        if fail_closed {
            self.attribute(recorded, &r.workspace_id).map(Some)
        } else {
            Ok(self.attribute_for_listing(recorded, &r.workspace_id))
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
    pub fn project_customer(&mut self, ws_id: &str) -> Result<Option<CustomerRef>, ApiError> {
        Ok(self.assigned(ws_id)?.map(|slug| self.customer_ref(&slug)))
    }
}

/// The provider account a customer's runs default to.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DefaultAccount {
    pub account: String,
    pub locked_by: Option<LockOwner>,
    /// `true` when the value comes from the global config, not the
    /// customer's layer.
    pub inherited: bool,
}

/// `(global config.toml mtime, customer config.toml mtime)`; `None` = absent.
type Stamps = (Option<SystemTime>, Option<SystemTime>);

fn mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// What one resolve of global + a customer's layer yields.
#[derive(Clone)]
struct CustomerLayer {
    pricing: PricingConfig,
    /// `Err` = the layers do not resolve (the message says why).
    default_account: Result<Option<DefaultAccount>, String>,
}

/// A customer's resolved config (global + that customer's layer, resolved
/// with `rupu_config::resolve`, locks honoured): its pricing — what its runs
/// are priced with — and its default account. Cached per slug and
/// re-validated by the mtimes of the global and customer `config.toml`, so
/// the files are read once per change, not once per request or customer.
///
/// The no-customer baseline follows the same rule: the global file is
/// re-resolved (global layer only) when its mtime moves, so a global pricing
/// edit reaches customer and no-customer runs alike. The startup snapshot is
/// only the fallback for a global file that does not parse.
pub struct CustomerPricing {
    global_dir: PathBuf,
    /// [`Self::flat`]: no files are read; every customer prices at `startup`.
    flat: bool,
    startup: PricingConfig,
    store: CustomerStore,
    baseline: Mutex<Option<(Option<SystemTime>, PricingConfig)>>,
    cache: Mutex<HashMap<String, (Stamps, CustomerLayer)>>,
}

impl CustomerPricing {
    pub fn new(global_dir: PathBuf, global: PricingConfig) -> Self {
        Self {
            store: CustomerStore::new(global_dir.clone()),
            global_dir,
            flat: false,
            startup: global,
            baseline: Mutex::new(None),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// One pricing for every customer, reading no files — for a registry or
    /// connector with no rupu home to resolve customer layers from (tests,
    /// an unwired default). Layers never fail here, so `layer_error` is
    /// always `None`.
    pub fn flat(pricing: PricingConfig) -> Self {
        Self {
            flat: true,
            ..Self::new(PathBuf::new(), pricing)
        }
    }

    /// The global-only pricing, re-resolved when the global `config.toml`
    /// mtime changes; the startup snapshot if the file fails to parse.
    fn global_pricing(&self) -> PricingConfig {
        if self.flat {
            return self.startup.clone();
        }
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

    /// `slug`'s resolved layer (valid slug only), from the cache when the
    /// files are unchanged.
    fn layer(&self, slug: &str) -> CustomerLayer {
        if self.flat {
            return CustomerLayer {
                pricing: self.startup.clone(),
                default_account: Ok(None),
            };
        }
        let global_path = self.global_dir.join("config.toml");
        let customer_path = self.store.config_path(slug);
        let stamps: Stamps = (mtime(&global_path), mtime(&customer_path));

        {
            let cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
            if let Some((cached, layer)) = cache.get(slug) {
                if *cached == stamps {
                    return layer.clone();
                }
            }
        }
        let layer = match rupu_config::resolve(LayerPaths::new(
            Some(&global_path),
            Some(&customer_path),
            None,
        )) {
            Ok(r) => {
                let prov = r.provenance.get("default_provider");
                let default_account =
                    r.config
                        .default_provider
                        .clone()
                        .map(|account| DefaultAccount {
                            account,
                            locked_by: prov.and_then(|p| p.locked_by),
                            inherited: prov.is_none_or(|p| p.source != KeySource::Customer),
                        });
                CustomerLayer {
                    pricing: r.config.pricing,
                    default_account: Ok(default_account),
                }
            }
            Err(e) => {
                tracing::warn!(
                    customer = slug,
                    error = %e,
                    "customer config layer unusable; using global pricing"
                );
                CustomerLayer {
                    pricing: self.global_pricing(),
                    default_account: Err(e.to_string()),
                }
            }
        };
        // The fallback is cached too, so a bad layer warns once per change.
        self.cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(slug.to_string(), (stamps, layer.clone()));
        layer
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
        self.layer(slug).pricing
    }

    /// Why `slug`'s work is priced at the global rates although it has a
    /// customer: its layer (global + `customers/<slug>/config.toml`) does not
    /// resolve. `None` when it resolves (a customer with no layer file
    /// resolves to the global pricing by design, which is no error) or for
    /// no customer. Cached with the layer, so this costs no extra read.
    pub fn layer_error(&self, slug: Option<&str>) -> Option<String> {
        let slug = slug?;
        if validate_slug(slug).is_err() {
            return None;
        }
        self.layer(slug)
            .default_account
            .err()
            .map(|e| format!("customer `{slug}`'s config layer does not resolve ({e}); its work is priced at the global rates"))
    }

    /// The customer's default provider account over global + its layer
    /// (`None` when neither sets one). `Err` = the layers do not resolve,
    /// with the reason.
    pub fn default_account(&self, slug: &str) -> Result<Option<DefaultAccount>, String> {
        validate_slug(slug).map_err(|e| e.to_string())?;
        self.layer(slug).default_account
    }
}

/// What prices one piece of work: the customer it is attributed to (`None`
/// = no customer), or `Unknown` — its attribution could not be read (an
/// unfiltered list degrades), priced at the global rates and flagged with
/// [`UNKNOWN_CUSTOMER_PRICING`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PriceKey {
    Customer(Option<String>),
    Unknown,
}

impl PriceKey {
    /// `Unknown` for `None` (the attribution can't be known), else the
    /// attributed customer.
    pub fn of(who: Option<&Attribution>) -> Self {
        match who {
            Some(w) => Self::Customer(w.slug.clone()),
            None => Self::Unknown,
        }
    }

    /// The customer whose pricing applies (`None` = global).
    pub fn slug(&self) -> Option<&str> {
        match self {
            Self::Customer(slug) => slug.as_deref(),
            Self::Unknown => None,
        }
    }

    /// The `pricing_error` work priced under this key carries.
    pub fn pricing_error(&self, prices: &mut dyn PriceBook) -> Option<String> {
        match self {
            Self::Customer(slug) => prices.pricing_error_for(slug.as_deref()),
            Self::Unknown => Some(UNKNOWN_CUSTOMER_PRICING.to_string()),
        }
    }
}

/// What prices a piece of work, by the customer it is attributed to
/// (`None` = no customer).
pub trait PriceBook {
    fn pricing_for(&mut self, customer: Option<&str>) -> &PricingConfig;

    /// Why `customer`'s work is priced at the global rates (its layer does
    /// not resolve) — what callers stamp on `UsageSummary.pricing_error`.
    /// `None` by default: a flat book has no layers to fail.
    fn pricing_error_for(&mut self, _customer: Option<&str>) -> Option<String> {
        None
    }
}

/// One pricing for every customer — for callers with no customer layers to
/// consult (a mirrored remote run, a unit test).
pub struct FlatPricing<'a>(pub &'a PricingConfig);

impl PriceBook for FlatPricing<'_> {
    fn pricing_for(&mut self, _customer: Option<&str>) -> &PricingConfig {
        self.0
    }
}

/// One request's pricing per customer: resolved once, then borrowed for
/// every run and transcript the request prices.
pub struct PricingMemo<'a> {
    pricing: &'a CustomerPricing,
    none: Option<PricingConfig>,
    by_slug: HashMap<String, PricingConfig>,
    errors: HashMap<String, Option<String>>,
}

impl PriceBook for PricingMemo<'_> {
    fn pricing_for(&mut self, customer: Option<&str>) -> &PricingConfig {
        self.get(customer)
    }

    fn pricing_error_for(&mut self, customer: Option<&str>) -> Option<String> {
        self.error(customer)
    }
}

impl<'a> PricingMemo<'a> {
    pub fn new(pricing: &'a CustomerPricing) -> Self {
        Self {
            pricing,
            none: None,
            by_slug: HashMap::new(),
            errors: HashMap::new(),
        }
    }

    /// [`CustomerPricing::layer_error`] for `slug`, memoized like the
    /// pricing.
    pub fn error(&mut self, slug: Option<&str>) -> Option<String> {
        let slug = slug?;
        let pricing = self.pricing;
        self.errors
            .entry(slug.to_string())
            .or_insert_with(|| pricing.layer_error(Some(slug)))
            .clone()
    }

    /// `u` priced for `slug`: stamps [`Self::error`] on it.
    pub fn stamp(
        &mut self,
        mut u: crate::usage::UsageSummary,
        slug: Option<&str>,
    ) -> crate::usage::UsageSummary {
        u.pricing_error = self.error(slug);
        u
    }

    /// The pricing for work attributed to `slug` (`None` = no customer).
    pub fn get(&mut self, slug: Option<&str>) -> &PricingConfig {
        let pricing = self.pricing;
        match slug {
            None => self.none.get_or_insert_with(|| pricing.for_customer(None)),
            Some(s) => {
                if !self.by_slug.contains_key(s) {
                    self.by_slug
                        .insert(s.to_string(), pricing.for_customer(Some(s)));
                }
                &self.by_slug[s]
            }
        }
    }
}

// ── remote rows (ruling 5) ────────────────────────────────────────────────

/// Response header a fan-out list sets, naming (comma-separated) the hosts
/// it skipped because some of their rows carry no `customer` key — hosts
/// that can't report a customer for every run (an older rupu, a mirror of a
/// worker's legacy runs, rows it could not attribute), which a customer
/// filter can neither keep nor drop.
pub const HOSTS_WITHOUT_CUSTOMER_HEADER: &str = "x-rupu-hosts-without-customer";

/// The 501 a single-host request answers when the host can't say whose some
/// of its runs are — a rupu older than customers, a mirror holding a worker's
/// legacy runs, or rows it could not attribute — the status
/// `host_list_error` gives an `Unsupported` host, so the web shows it as
/// "unavailable".
pub fn customers_unsupported(host_id: &str) -> ApiError {
    ApiError::not_available(format!(
        "host {host_id} can't report a customer for every run"
    ))
}

/// The 501 for an aggregate (`/api/usage`, `/api/dashboard`) asked of a
/// remote host with a customer filter: a remote aggregate arrives already
/// summed, so the coordinator cannot filter it (ruling 5).
pub fn remote_aggregate_unsupported(host_id: &str) -> ApiError {
    ApiError::not_available(remote_aggregate_reason(host_id))
}

/// The reason a fan-out aggregate gives for a remote host it leaves out
/// under a customer filter.
pub fn remote_aggregate_reason(host_id: &str) -> String {
    format!("host {host_id} can't be filtered by customer: its totals are summed remotely")
}

/// Keep the remote `rows` whose `customer` matches `filter`. `None` when any
/// row lacks the `customer` key, or carries a value that is neither a string
/// nor `null` (the host can't say whose that run is — never read as "no
/// customer"); a `null` value is "no customer". An empty page is filterable
/// and stays empty.
pub fn filter_remote_rows(
    rows: Vec<serde_json::Value>,
    filter: &CustomerFilter,
) -> Option<Vec<serde_json::Value>> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let customer = match row.as_object()?.get("customer")? {
            serde_json::Value::Null => None,
            serde_json::Value::String(s) => Some(s.as_str()),
            // A slug is a string: anything else is a row this coordinator
            // cannot read, not "no customer".
            _ => return None,
        };
        if filter.matches(customer) {
            out.push(row);
        }
    }
    Some(out)
}

/// Record `r`'s worker as a host that can't report a customer for every
/// run (a legacy mirrored run left out of a filter, count or rollup), once.
pub fn note_unreportable(hosts: &mut Vec<String>, r: &rupu_orchestrator::RunRecord) {
    if let Some(w) = r.worker_id.as_deref() {
        if !hosts.iter().any(|h| h == w) {
            hosts.push(w.to_string());
        }
    }
}

/// The fan-out header value for the hosts skipped, if any.
pub fn hosts_without_customer_header(hosts: &[String]) -> axum::http::HeaderMap {
    let mut headers = axum::http::HeaderMap::new();
    if hosts.is_empty() {
        return headers;
    }
    if let Ok(v) = axum::http::HeaderValue::from_str(&hosts.join(",")) {
        headers.insert(HOSTS_WITHOUT_CUSTOMER_HEADER, v);
    }
    headers
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

    /// A remote row is filterable by a string or `null` `customer`; a row
    /// without the key — or with any other value — makes the page
    /// unfilterable (the host can't report), never "no customer".
    #[test]
    fn remote_rows_fail_closed_on_a_missing_or_malformed_customer() {
        use serde_json::json;
        let acme = CustomerFilter::Slug("acme".into());
        let none = CustomerFilter::Unassigned;
        let rows = vec![
            json!({"id": "a", "customer": "acme"}),
            json!({"id": "n", "customer": null}),
        ];
        assert_eq!(filter_remote_rows(rows.clone(), &acme).unwrap().len(), 1);
        assert_eq!(filter_remote_rows(rows, &none).unwrap()[0]["id"], "n");
        for bad in [
            json!(7),
            json!({"slug": "acme"}),
            json!(["acme"]),
            json!(false),
        ] {
            let rows = vec![json!({"id": "x", "customer": bad.clone()})];
            assert!(filter_remote_rows(rows.clone(), &none).is_none(), "{bad}");
            assert!(filter_remote_rows(rows, &acme).is_none(), "{bad}");
        }
        assert!(filter_remote_rows(vec![json!({"id": "x"})], &none).is_none());
        assert_eq!(filter_remote_rows(vec![], &none), Some(vec![]));
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

        let a = attribute(&store, Recorded::Slug("zed"), "ws_assigned").unwrap();
        assert_eq!((a.slug.as_deref(), a.derived), (Some("zed"), false));

        // Only a LEGACY record (no key) derives from the current assignment.
        let a = attribute(&store, Recorded::Legacy, "ws_assigned").unwrap();
        assert_eq!((a.slug.as_deref(), a.derived), (Some("acme"), true));

        // A recorded "no customer" stays none even though the project is
        // assigned now: reassigning a project never rewrites history.
        let a = attribute(&store, Recorded::None, "ws_assigned").unwrap();
        assert_eq!((a.slug, a.derived), (None, false));

        let a = attribute(&store, Recorded::Legacy, "ws_unassigned").unwrap();
        assert_eq!((a.slug, a.derived), (None, false));

        // A workspace id the store rejects never fails a listing.
        let a = attribute(&store, Recorded::Legacy, "../weird id").unwrap();
        assert_eq!((a.slug, a.derived), (None, false));

        // A session-inherited slug is derived; an inherited none is not.
        assert!(
            attribute(&store, Recorded::Slug("zed"), "ws_assigned")
                .unwrap()
                .inherited(true)
                .derived
        );
        assert!(
            !attribute(&store, Recorded::None, "ws_assigned")
                .unwrap()
                .inherited(true)
                .derived
        );
    }

    #[test]
    fn session_turn_customer_takes_the_session_only_for_a_legacy_turn() {
        let slug = |s: &str| Some(Some(s.to_string()));
        assert_eq!(
            session_turn_customer(&slug("a"), &slug("b")),
            (slug("a"), false)
        );
        assert_eq!(
            session_turn_customer(&Some(None), &slug("b")),
            (Some(None), false)
        );
        assert_eq!(session_turn_customer(&None, &slug("b")), (slug("b"), true));
        assert_eq!(
            session_turn_customer(&None, &Some(None)),
            (Some(None), true)
        );
        assert_eq!(session_turn_customer(&None, &None), (None, false));
    }

    #[test]
    fn lookup_memoizes_and_matches_attribute() {
        let tmp = tempfile::tempdir().unwrap();
        let store = assigned_store(tmp.path());
        let mut lookup = CustomerLookup::new(store.clone());

        for (recorded, ws) in [
            (Recorded::Slug("zed"), "ws_assigned"),
            (Recorded::None, "ws_assigned"),
            (Recorded::Legacy, "ws_assigned"),
            (Recorded::Legacy, "ws_unassigned"),
            (Recorded::Legacy, "../weird id"),
        ] {
            assert_eq!(
                lookup.attribute(recorded, ws).unwrap(),
                attribute(&store, recorded, ws).unwrap()
            );
        }

        // Memoized: removing the sidecar does not change this request's view.
        std::fs::remove_file(tmp.path().join("workspaces/ws_assigned.customer")).unwrap();
        assert_eq!(
            lookup.assigned("ws_assigned").unwrap().as_deref(),
            Some("acme")
        );
        assert_eq!(
            CustomerLookup::new(store.clone())
                .assigned("ws_assigned")
                .unwrap(),
            None
        );

        let r = lookup.project_customer("ws_assigned").unwrap().unwrap();
        assert_eq!((r.slug.as_str(), r.name.as_str()), ("acme", "Acme"));
        // A dangling slug still shows, named by its slug.
        let d = lookup.customer_ref("gone");
        assert_eq!((d.slug.as_str(), d.name.as_str()), ("gone", "gone"));
    }

    #[test]
    fn an_unreadable_assignment_is_an_error_not_unassigned() {
        let tmp = tempfile::tempdir().unwrap();
        let store = assigned_store(tmp.path());
        // A directory where the sidecar file belongs: read fails with an
        // error other than NotFound.
        std::fs::create_dir(tmp.path().join("workspaces/ws_broken.customer")).unwrap();

        let err = attribute(&store, Recorded::Legacy, "ws_broken").unwrap_err();
        assert_eq!(err.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(err.1.contains("ws_broken"), "{}", err.1);
        // A recorded customer never needs the sidecar.
        assert_eq!(
            attribute(&store, Recorded::Slug("acme"), "ws_broken")
                .unwrap()
                .slug
                .as_deref(),
            Some("acme")
        );
        // Nor does a recorded none.
        assert_eq!(
            attribute(&store, Recorded::None, "ws_broken").unwrap().slug,
            None
        );
        let err = CustomerLookup::new(store)
            .attribute(Recorded::Legacy, "ws_broken")
            .unwrap_err();
        assert_eq!(err.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(err.1.contains("ws_broken"), "{}", err.1);
    }

    #[test]
    fn default_account_is_resolved_and_cached_with_the_layer() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::write(
            home.join("config.toml"),
            "default_provider = \"anthropic\"\n",
        )
        .unwrap();
        let store = CustomerStore::new(home);
        for slug in ["acme", "globex"] {
            store
                .create(
                    slug,
                    &NewCustomer {
                        name: slug.into(),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        std::fs::write(
            store.config_path("acme"),
            "default_provider = \"anthropic-acme\"\n[policy]\nlock = [\"default_provider\"]\n",
        )
        .unwrap();
        let pricing = CustomerPricing::new(home.to_path_buf(), PricingConfig::default());
        assert_eq!(
            pricing.default_account("acme").unwrap(),
            Some(DefaultAccount {
                account: "anthropic-acme".into(),
                locked_by: Some(LockOwner::Customer),
                inherited: false,
            })
        );
        assert_eq!(
            pricing.default_account("globex").unwrap(),
            Some(DefaultAccount {
                account: "anthropic".into(),
                locked_by: None,
                inherited: true,
            })
        );
        std::fs::write(store.config_path("globex"), "= = nope").unwrap();
        set_mtime(
            &store.config_path("globex"),
            SystemTime::now() + Duration::from_secs(60),
        );
        assert!(pricing.default_account("globex").is_err());
        assert!(pricing.default_account("Bad Slug").is_err());
    }

    /// A malformed layer falls back to global pricing and says why (naming
    /// the customer); a customer with no layer file, or no customer, is no
    /// error. The memo stamps it on a summary.
    #[test]
    fn a_malformed_layer_is_reported_as_a_pricing_error() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let store = CustomerStore::new(home);
        for slug in ["acme", "globex"] {
            store
                .create(
                    slug,
                    &NewCustomer {
                        name: slug.into(),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        std::fs::write(store.config_path("acme"), "= = nope").unwrap();
        std::fs::remove_file(store.config_path("globex")).unwrap();
        let pricing = CustomerPricing::new(home.to_path_buf(), PricingConfig::default());
        let err = pricing.layer_error(Some("acme")).unwrap();
        assert!(
            err.contains("`acme`") && err.contains("global rates"),
            "{err}"
        );
        assert_eq!(pricing.layer_error(Some("globex")), None, "no layer file");
        assert_eq!(pricing.layer_error(None), None);

        let mut memo = PricingMemo::new(&pricing);
        let u = memo.stamp(crate::usage::UsageSummary::default(), Some("acme"));
        assert_eq!(u.pricing_error.as_deref(), Some(err.as_str()));
        assert_eq!(memo.pricing_error_for(Some("globex")), None);
        // The rollup keeps it.
        let merged = crate::usage::rollup([crate::usage::UsageSummary::default(), u].into_iter());
        assert!(merged.pricing_error.is_some());
    }

    #[test]
    fn pricing_memo_resolves_once_per_customer() {
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
        write_layer(&store.config_path("acme"), 7.0);
        let pricing = CustomerPricing::new(home.to_path_buf(), PricingConfig::default());
        let mut memo = PricingMemo::new(&pricing);
        assert_eq!(price(memo.get(Some("acme"))), Some(7.0));
        assert_eq!(price(memo.get(None)), None);
        // The memo holds this request's view even if the layer changes.
        write_layer(&store.config_path("acme"), 9.0);
        set_mtime(
            &store.config_path("acme"),
            SystemTime::now() + Duration::from_secs(60),
        );
        assert_eq!(price(memo.get(Some("acme"))), Some(7.0));
        assert_eq!(price(&pricing.for_customer(Some("acme"))), Some(9.0));
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
