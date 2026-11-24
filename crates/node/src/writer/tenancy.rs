//! What an index's descriptor settings say about whom it bills to: its
//! bucket group, its cost tier and its cost budget.
//!
//! The settings are flat JSON (`"index.serverless.bucket_group":"acme"`) or
//! nested, as the Java writes them.

use oss_writer::mapping::json::{Json, parse};

use super::publish_age::find;

const GROUP_KEY: &str = "index.serverless.bucket_group";
/// `index.serverless.cost.tier`.
pub const COST_TIER: &str = "index.serverless.cost.tier";
/// `index.serverless.cost.budget`.
pub const COST_BUDGET: &str = "index.serverless.cost.budget";
use COST_BUDGET as BUDGET_KEY;
use COST_TIER as TIER_KEY;

/// An index's billing settings; a field is `None` when the index sets none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tenancy {
    /// The group the index is bucketed with, which is its tenant.
    pub group: Option<String>,
    /// `small`, `standard` or `large`.
    pub tier: Option<String>,
    /// Cost units a minute; wins over the tier.
    pub budget: Option<u64>,
}

impl Tenancy {
    /// Whether the index sets none of them.
    pub fn is_empty(&self) -> bool {
        self.group.is_none() && self.tier.is_none() && self.budget.is_none()
    }
}

/// The billing settings of a descriptor's settings JSON; text that is not
/// JSON, or a value that does not parse, reads as unset.
#[must_use]
pub fn tenancy_of(settings: &[u8]) -> Tenancy {
    let Ok(Json::Obj(members)) = parse(settings) else {
        return Tenancy::default();
    };
    let text = |key: &str| match find(&members, key) {
        // ALLOC: one name, once a shard opened.
        Some(Json::Str(value) | Json::Num(value)) if !value.is_empty() => Some(value.to_string()),
        _ => None,
    };
    Tenancy {
        group: text(GROUP_KEY),
        tier: text(TIER_KEY),
        budget: text(BUDGET_KEY)
            .and_then(|b| b.trim().parse::<u64>().ok())
            .filter(|b| *b > 0),
    }
}

/// The descriptor settings JSON of an index with `tenancy`.
#[must_use]
pub fn tenancy_settings(tenancy: &Tenancy) -> Vec<u8> {
    // ALLOC: one small body, once an index created.
    let mut out = vec![b'{'];
    let mut first = true;
    let mut member = |key: &str, value: &dyn Fn(&mut Vec<u8>)| {
        if !first {
            out.push(b',');
        }
        first = false;
        let _ = oss_rest::write_json_str(&mut out, key);
        out.push(b':');
        value(&mut out);
    };
    if let Some(group) = &tenancy.group {
        member(GROUP_KEY, &|o| {
            let _ = oss_rest::write_json_str(o, group);
        });
    }
    if let Some(tier) = &tenancy.tier {
        member(TIER_KEY, &|o| {
            let _ = oss_rest::write_json_str(o, tier);
        });
    }
    if let Some(budget) = tenancy.budget {
        member(BUDGET_KEY, &|o| {
            o.extend_from_slice(budget.to_string().as_bytes())
        });
    }
    out.push(b'}');
    out
}
