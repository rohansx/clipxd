//! Browser-salience model (`docs/phase2-browser-spec.md` §3).
//!
//! No pixels are scored. `salience = clamp(w_focus · magnitude · novelty)`, emitted to the
//! visual timeline when `>= salience_min` — the same gate shape as veyo's pixel path,
//! recast over the event stream. Captions are deterministic templates from typed fields.
//!
//! The headline feature is the **gesture → request join** (§3.6): a click followed within
//! `causal_window_ms` by a request becomes one line — *"Clicked \"Place order\" → POST
//! /api/checkout (500)"* — the single best answer to "what was the user doing right before
//! the error". When a request is named by such a join, its standalone `network_error` line
//! is suppressed (coalescing, §3.8).

use crate::trace::{network_is_error, BrowserTrace, TraceEvent};
use std::collections::HashMap;

/// Tunable knobs (defaults mirror veyo + browser-specific additions).
#[derive(Debug, Clone)]
pub struct Opts {
    pub salience_min: f32,
    pub novelty_decay: f32,
    pub causal_window_ms: u64,
    pub size_norm: f32,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            salience_min: 0.4,
            novelty_decay: 0.9,
            causal_window_ms: 1500,
            size_norm: 12.0,
        }
    }
}

/// A salient moment derived from the event stream.
#[derive(Debug, Clone)]
pub struct Moment {
    pub t: f64,
    pub salience: f32,
    pub caption: String,
    pub delta: String,
}

/// Per-signature novelty with habituation + network pattern-break.
struct Novelty {
    map: HashMap<String, f32>,
    net_class: HashMap<String, u16>,
    decay: f32,
}

impl Novelty {
    fn new(decay: f32) -> Self {
        Self { map: HashMap::new(), net_class: HashMap::new(), decay }
    }
    /// Novelty for `sig` (1.0 first time), then decay it for next time.
    fn observe(&mut self, sig: &str) -> f32 {
        let n = *self.map.get(sig).unwrap_or(&1.0);
        self.map.insert(sig.to_string(), n * self.decay);
        n
    }
    /// Network novelty, keyed by (method, url_template, status_class). A route that was
    /// habituated returning one status class and now returns another (e.g. 200s → 500)
    /// is a **pattern-break**: reset that signature's novelty toward 1.0 so it fires.
    fn observe_net(&mut self, method: &str, templ: &str, sclass: u16) -> f32 {
        let route = format!("{method} {templ}");
        let broke = self.net_class.get(&route).is_some_and(|prev| *prev != sclass);
        self.net_class.insert(route, sclass);
        let sig = format!("net {method} {templ} {sclass}");
        if broke {
            self.map.insert(sig.clone(), 1.0);
        }
        self.observe(&sig)
    }
}

fn emit(out: &mut Vec<Moment>, t: f64, w: f32, mag: f32, novelty: f32, caption: String, delta: &str, min: f32) {
    let s = (w * mag * novelty).clamp(0.0, 1.0);
    if s >= min {
        out.push(Moment { t, salience: s, caption, delta: delta.into() });
    }
}

/// Derive the salient `visual_timeline` moments from a trace.
pub fn derive(trace: &BrowserTrace, opts: &Opts) -> Vec<Moment> {
    let mut order: Vec<usize> = (0..trace.events.len()).collect();
    order.sort_by_key(|&i| trace.events[i].t_ms().unwrap_or(u64::MAX));

    let mut nov = Novelty::new(opts.novelty_decay);
    let mut last_click: Option<(u64, String)> = None; // (t_ms, label)
    let mut out = Vec::new();

    for &i in &order {
        let ev = &trace.events[i];
        let Some(t_ms) = ev.t_ms() else { continue };
        let t = trace.rel_s(t_ms);

        match ev {
            TraceEvent::Click { label, target, .. } => {
                let l = label.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| target.clone());
                last_click = Some((t_ms, l));
            }

            TraceEvent::Network { method, url, status, status_text, duration_ms, error_text, resource_type, .. } => {
                let templ = url_template(url);
                let sclass = status.map(|s| s / 100).unwrap_or(0);
                let novelty = nov.observe_net(method, &templ, sclass);
                let is_err = network_is_error(*status, error_text);

                // gesture → request join (consumes the pending click). Only a *primary*
                // request (a fetch/xhr/document, or an unknown type) joins to a click; a
                // background asset (image/css/font/script/media) must NOT be mislabeled as
                // "caused by" the last click. NB: this remains a time+type heuristic — it
                // does not prove causation (no request_id↔click correlation in v1), so a
                // sync request shortly after an unrelated click can still join. The raw
                // click + network rows stay separate in `event_track` regardless.
                let primary = resource_type.is_empty()
                    || matches!(resource_type.as_str(), "fetch" | "xhr" | "document");
                let joined = match &last_click {
                    Some((ct, label)) if primary && t_ms.saturating_sub(*ct) <= opts.causal_window_ms => {
                        Some(label.clone())
                    }
                    _ => None,
                };
                if let Some(label) = joined {
                    let mag = if is_err { net_magnitude(*status, error_text) } else { 0.7 };
                    let st = status
                        .map(|s| s.to_string())
                        .or_else(|| error_text.clone())
                        .unwrap_or_else(|| "failed".into());
                    let cap = format!("Clicked \"{label}\" → {method} {} ({st})", path_of(url));
                    emit(&mut out, t, 1.5, mag, novelty, cap, "gesture_request", opts.salience_min);
                    last_click = None;
                } else if is_err {
                    // standalone error (no gesture named it)
                    let mag = net_magnitude(*status, error_text);
                    let cap = match (status, error_text) {
                        (Some(s), _) => format!(
                            "{method} {} → {s}{}{}",
                            path_of(url),
                            status_text.as_deref().map(|x| format!(" ({x})")).unwrap_or_default(),
                            duration_ms.map(|d| format!(" ({d}ms)")).unwrap_or_default(),
                        ),
                        (None, Some(e)) => format!("{method} {} failed: {e}", path_of(url)),
                        _ => format!("{method} {} failed", path_of(url)),
                    };
                    emit(&mut out, t, 1.4, mag, novelty, cap, "network_error", opts.salience_min);
                }
            }

            TraceEvent::Console { level, text, uncaught, stack, .. } => {
                let mag = if *uncaught {
                    1.0
                } else {
                    match level.as_str() {
                        "error" | "assert" => 0.9,
                        "warn" | "warning" => 0.5,
                        _ => continue, // plain logs aren't salient moments
                    }
                };
                let novelty = nov.observe(&format!("console {level} {}", msg_signature(text)));
                let cap = if *uncaught {
                    let at = stack.as_ref().and_then(|s| s.first()).map(|f| format!(" (at {f})")).unwrap_or_default();
                    format!("Uncaught: {}{}", truncate(text, 80), at)
                } else {
                    format!("Console error: {}", truncate(text, 80))
                };
                emit(&mut out, t, 1.5, mag, novelty, cap, "console_error", opts.salience_min);
            }

            TraceEvent::Navigate { url, title, nav_kind, .. } => {
                let novelty = nov.observe(&format!("nav {}", url_template(url)));
                let mag = if nav_kind == "load" { 1.0 } else { 0.9 };
                let verb = match nav_kind.as_str() {
                    "reload" => "Reloaded",
                    "back_forward" => "Back to",
                    _ => "Navigated to",
                };
                let title_s = title.as_deref().filter(|s| !s.is_empty()).map(|t| format!(" — {t}")).unwrap_or_default();
                let cap = format!("{verb} {}{title_s}", path_of(url));
                emit(&mut out, t, 1.6, mag, novelty, cap, "navigation", opts.salience_min);
            }

            TraceEvent::DomMutation { target, op, added, removed, text_delta, role, name, attr, .. } => {
                if op == "attr" && matches!(attr.as_deref(), Some("class") | Some("style")) {
                    continue; // cosmetic only
                }
                let role_s = role.as_deref().unwrap_or("");
                let alertish = matches!(role_s, "alert" | "alertdialog" | "dialog" | "status");
                let mut mag = (((*added + *removed) as f32 + (text_delta.unsigned_abs() as f32) / 200.0)
                    / opts.size_norm)
                    .clamp(0.0, 1.0);
                if alertish || op == "replace" {
                    mag = mag.max(0.9);
                }
                let novelty = nov.observe(&format!("dom {target} {op}"));
                let nm = name.as_deref().unwrap_or("").trim();
                let (delta, cap) = if matches!(role_s, "dialog" | "alertdialog") {
                    ("modal_opened", format!("Dialog \"{nm}\" opened"))
                } else if op == "replace" {
                    ("dom_subtree_replaced", format!("Replaced {target} subtree (new view)"))
                } else if alertish && !nm.is_empty() {
                    ("node_inserted", format!("{role_s} inserted into {target}: \"{}\"", truncate(nm, 60)))
                } else if nm.is_empty() {
                    ("dom_mutation", format!("DOM updated in {target}"))
                } else {
                    ("dom_mutation", format!("{target}: \"{}\"", truncate(nm, 60)))
                };
                emit(&mut out, t, 1.0, mag, novelty, cap, delta, opts.salience_min);
            }

            TraceEvent::Screenshot { reason, .. } if reason == "state_settle" => {
                out.push(Moment { t, salience: 0.6, caption: "Page settled".into(), delta: "state_settle".into() });
            }

            _ => {}
        }
    }
    out
}

fn net_magnitude(status: Option<u16>, error_text: &Option<String>) -> f32 {
    match status {
        Some(s) if s >= 500 => 1.0,
        Some(401) | Some(403) => 0.85,
        Some(s) if s >= 400 => 0.8,
        _ => {
            if error_text.is_some() {
                0.9
            } else {
                0.6
            }
        }
    }
}

/// The path portion of a URL (drops scheme://host and ?query#frag).
fn path_of(url: &str) -> String {
    let rest = match url.find("://") {
        Some(i) => &url[i + 3..],
        None => url,
    };
    let path = match url.find("://").and(rest.find('/')) {
        Some(j) => &rest[j..],
        None => "/",
    };
    path.split(['?', '#']).next().unwrap_or(path).to_string()
}

/// Normalize a URL path so polling/REST ids habituate: numeric or uuid/hex-ish segments
/// → `:id`.
fn url_template(url: &str) -> String {
    path_of(url)
        .split('/')
        .map(|s| {
            let id_like = !s.is_empty()
                && (s.chars().all(|c| c.is_ascii_digit())
                    || (s.len() >= 8 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')));
            if id_like {
                ":id"
            } else {
                s
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Console-message signature for novelty: digits collapsed so "HTTP 500 at /x/123" and
/// "HTTP 503 at /x/999" share a baseline.
fn msg_signature(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_digit() { '#' } else { c }).collect()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n).collect();
        t.push('…');
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::BrowserTrace;

    fn trace(events: serde_json::Value) -> BrowserTrace {
        let v = serde_json::json!({ "started_at_ms": 0, "url": "https://x/", "events": events });
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn gesture_request_join_names_cause_and_effect() {
        let tr = trace(serde_json::json!([
            {"type":"click","t_ms":1000,"target":"button#place-order","label":"Place order"},
            {"type":"network","t_ms":1100,"method":"POST","url":"https://x/api/checkout","status":500,"status_text":"Internal Server Error"}
        ]));
        let m = derive(&tr, &Opts::default());
        let g = m.iter().find(|m| m.delta == "gesture_request").expect("join moment");
        assert!(g.caption.contains("Place order") && g.caption.contains("POST /api/checkout") && g.caption.contains("500"), "{}", g.caption);
        // standalone network_error suppressed (coalesced into the join)
        assert!(!m.iter().any(|m| m.delta == "network_error"), "standalone network_error should be suppressed");
        assert!(g.salience >= 0.9);
    }

    #[test]
    fn background_asset_does_not_join_to_a_click() {
        // a click then a background IMAGE 500 must NOT produce a gesture_request
        let tr = trace(serde_json::json!([
            {"type":"click","t_ms":1000,"target":"a#logo","label":"Home"},
            {"type":"network","t_ms":1100,"method":"GET","url":"https://x/banner.png","status":500,"resource_type":"image"}
        ]));
        let m = derive(&tr, &Opts::default());
        assert!(!m.iter().any(|m| m.delta == "gesture_request"), "image load must not join to the click");
        // it is still recorded as a standalone network error
        assert!(m.iter().any(|m| m.delta == "network_error"));
    }

    #[test]
    fn rapid_clicks_join_to_the_most_recent() {
        let tr = trace(serde_json::json!([
            {"type":"click","t_ms":1000,"target":"a#products","label":"Products"},
            {"type":"click","t_ms":1100,"target":"button#search","label":"Search"},
            {"type":"network","t_ms":1200,"method":"GET","url":"https://x/api/search","status":200,"resource_type":"fetch"}
        ]));
        let g = derive(&tr, &Opts::default()).into_iter().find(|m| m.delta == "gesture_request");
        // documents current behavior: the latest click within the window wins
        assert!(g.map(|g| g.caption.contains("Search")).unwrap_or(false));
    }

    #[test]
    fn alert_toast_is_salient_node_inserted() {
        let tr = trace(serde_json::json!([
            {"type":"dom_mutation","t_ms":500,"target":"#notifications","op":"insert","added":1,"role":"alert","name":"Payment failed (500)"}
        ]));
        let m = derive(&tr, &Opts::default());
        let n = m.iter().find(|m| m.delta == "node_inserted").expect("toast moment");
        assert!(n.caption.contains("Payment failed (500)") && n.salience >= 0.9, "{}", n.caption);
    }

    #[test]
    fn spinner_habituates_does_not_flood() {
        // 30 identical alert insertions: a few fire, then novelty drives it silent.
        let evs: Vec<_> = (0..30).map(|k| serde_json::json!(
            {"type":"dom_mutation","t_ms": 100*k, "target":"#app","op":"insert","added":1,"role":"status","name":"loading"}
        )).collect();
        let m = derive(&trace(serde_json::json!(evs)), &Opts::default());
        assert!(m.len() < 12, "habituation should bound emissions, got {}", m.len());
        assert!(!m.is_empty(), "the first occurrences should still fire");
    }

    #[test]
    fn network_pattern_break_still_fires_after_habituation() {
        // /poll returns 200 many times (no moments), then a 500 — pattern-break must fire.
        let mut evs: Vec<serde_json::Value> = (0..8).map(|k| serde_json::json!(
            {"type":"network","t_ms":100*k,"method":"GET","url":"https://x/poll","status":200}
        )).collect();
        evs.push(serde_json::json!({"type":"network","t_ms":2000,"method":"GET","url":"https://x/poll","status":500}));
        let m = derive(&trace(serde_json::json!(evs)), &Opts::default());
        assert!(m.iter().any(|m| m.delta == "network_error" && m.caption.contains("500")),
            "pattern-break 200->500 must emit, got {m:?}");
    }
}
