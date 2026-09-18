//! Shared test fixture: a small, neutral board (10 cards across the columns).
#![allow(dead_code)]

use terminal_board::store::{Result, Seed, Store};

const M: i64 = 60;
const H: i64 = 3600;
const D: i64 = 86400;

pub fn seed(store: &Store, you: &str) -> Result<Vec<i64>> {
    let tomorrow = (chrono::Local::now() + chrono::Duration::days(1)).format("%b %-d").to_string();
    let seeds = [
        Seed {
            title: "widgets: gh#308 csv export",
            desc: "CSV export drops the header row. Done = header present, test added.",
            column: "todo",
            owner: None,
            age_secs: 3 * D,
            due: None,
            checks: &[],
            notes: &[],
        },
        Seed {
            title: "widgets: gh#309 retry backoff",
            desc: "Retry failed uploads with exponential backoff.",
            column: "todo",
            owner: None,
            age_secs: 2 * D,
            due: None,
            checks: &[("pick the limits", false), ("wire into uploader", false)],
            notes: &[],
        },
        Seed {
            title: "ops: rotate API tokens",
            desc: "Rotate the deploy and CI tokens before they expire.",
            column: "todo",
            owner: None,
            age_secs: D + 2 * H,
            due: Some(tomorrow.as_str()),
            checks: &[],
            notes: &[],
        },
        Seed {
            title: "admin: renew domain",
            desc: "Renew before the end of the month; check auto-renew.",
            column: "todo",
            owner: None,
            age_secs: D,
            due: None,
            checks: &[],
            notes: &[],
        },
        Seed {
            title: "widgets: gh#327 login form rejects valid emails",
            desc: "Emails with a plus sign are rejected. Done = plus-addresses accepted,\ntest added, PR green.",
            column: "doing",
            owner: Some("bot-2"),
            age_secs: 40 * M,
            due: None,
            checks: &[
                ("reproduce with a+b@example.com", true),
                ("patch validator", true),
                ("add test", false),
                ("open PR", false),
            ],
            notes: &[("bot-2", "repro confirmed"), ("bot-2", "patch applied, tests running")],
        },
        Seed {
            title: "admin: vendor quote",
            desc: "Get the final quote from the hosting vendor.",
            column: "doing",
            owner: Some(you),
            age_secs: 2 * D,
            due: None,
            checks: &[],
            notes: &[(you, "left a voicemail")],
        },
        Seed {
            title: "widgets: gh#314 search index lags",
            desc: "New items take minutes to show in search. PR #334 is up; review re-running.",
            column: "review",
            owner: Some("rev"),
            age_secs: 12 * M,
            due: None,
            checks: &[("fix", true), ("test", true), ("CI green", false)],
            notes: &[("rev", "CI running")],
        },
        Seed {
            title: "widgets: gh#325 typo in footer",
            desc: "",
            column: "done",
            owner: Some("bot"),
            age_secs: 3 * H,
            due: None,
            checks: &[],
            notes: &[("bot", "PR #333 merged")],
        },
        Seed {
            title: "widgets: gh#328 dark theme",
            desc: "",
            column: "done",
            owner: Some("bot-3"),
            age_secs: 5 * H,
            due: None,
            checks: &[],
            notes: &[("bot-3", "PR #332 merged")],
        },
        Seed {
            title: "fix README images",
            desc: "",
            column: "done",
            owner: Some(you),
            age_secs: 6 * H,
            due: None,
            checks: &[],
            notes: &[],
        },
    ];
    let ids = seeds.iter().map(|s| store.seed(s)).collect::<Result<Vec<_>>>()?;
    store.order_by_time()?;
    // retry backoff waits on the search fix in review
    store.block(ids[1], Some(&format!("#{}", ids[6])), "seed")?;
    Ok(ids)
}
