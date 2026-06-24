//! Trace → clipxd [`Index`] (`docs/phase2-browser-spec.md` §2).
//!
//! Two passes: a **lossless** pass (every event → `event_track`; DOM text → `on_screen_text`
//! verbatim with `source:"dom"`, `bbox:null`; masked values → `redaction.items`, never
//! copied), then the salience pass ([`crate::salience`]) for `visual_timeline` + `summary`.
//! The output is the *same* `Index` shape as Phase 1 — only `source:"browser"` differs.

use crate::salience::{self, Opts};
use crate::trace::{network_is_error, BrowserTrace, TraceEvent};
use clipxd_index::{
    Chapter, Event, Index, Metadata, OnScreenText, Redaction, RedactionItem, Source, Summary,
    TextKind, VisualMoment,
};
use serde_json::json;

const EPS_S: f64 = 2.0; // open window for instantaneous messages

// Defensive caps so a hostile or pathological trace can't OOM the ingestor. Truncation is
// logged (never silent) per the spec's "no silent caps" rule.
const MAX_SNAPSHOT_LINES: usize = 300;
const MAX_TEXT_CHARS: usize = 600;
const MAX_ON_SCREEN_TEXT: usize = 8_000;
const MAX_VISUAL_TIMELINE: usize = 4_000;
const MAX_EVENT_TRACK: usize = 100_000;
const MAX_REDACTION: usize = 8_000;

/// Build the clip [`Index`] from a parsed trace.
pub fn build_index(trace: &BrowserTrace, id: &str, created_at: &str, opts: &Opts) -> Index {
    let end = trace.duration_s();
    let url_context = first_url(trace);
    let title = page_title(trace).unwrap_or_else(|| url_context.clone());

    let mut idx = Index::new(
        id,
        Source::Browser,
        Metadata {
            duration: end,
            resolution: [trace.viewport.w, trace.viewport.h],
            fps: 0.0,
            created_at: created_at.to_string(),
            title,
            app_focus: Vec::new(),
            url_context: Some(url_context),
            has_video: false,
        },
    );

    let mut order: Vec<usize> = (0..trace.events.len()).collect();
    order.sort_by_key(|&i| trace.events[i].t_ms().unwrap_or(u64::MAX));

    let mut redactions: Vec<RedactionItem> = Vec::new();
    for &i in &order {
        let ev = &trace.events[i];
        let Some(t_ms) = ev.t_ms() else { continue };
        let t = trace.rel_s(t_ms);
        if let Some(e) = to_event(ev, t) {
            idx.event_track.push(e);
        }
        push_on_screen_text(&mut idx, ev, t, end);
        push_redaction(&mut redactions, ev, t);
    }

    for m in salience::derive(trace, opts) {
        idx.visual_timeline.push(VisualMoment {
            t: m.t,
            salience: m.salience,
            caption: m.caption,
            delta: m.delta,
            frame_ref: None,
        });
    }
    attach_frames(&mut idx, trace);

    // Defensive caps (logged) before deriving the summary so it reflects the capped data.
    cap_stream(&mut idx.event_track, MAX_EVENT_TRACK, "event_track");
    cap_stream(&mut idx.on_screen_text, MAX_ON_SCREEN_TEXT, "on_screen_text");
    cap_stream(&mut idx.visual_timeline, MAX_VISUAL_TIMELINE, "visual_timeline");
    cap_stream(&mut redactions, MAX_REDACTION, "redaction");

    idx.summary = derive_summary(&idx);
    if !redactions.is_empty() {
        idx.redaction = Redaction {
            ran: true,
            engine: Some("capture-dom".into()),
            items: redactions,
            // capture-side DOM masking handles form fields; arbitrary secrets in console /
            // network / DOM *text* are scanned by CloakPipe in Phase 4 (not yet enforced).
            policy: "dom-mask-phase2; text-scan-deferred-phase4".into(),
        };
    }
    idx
}

/// §2.1 — lossless event_track mapping. Returns `None` for structural events
/// (`dom_snapshot`, `screenshot`, `a11y_text`, `unknown`).
fn to_event(ev: &TraceEvent, t: f64) -> Option<Event> {
    use TraceEvent::*;
    let e = match ev {
        Navigate { url, from, nav_kind, title, .. } => Event {
            t,
            kind: "navigation".into(),
            text: Some(url.clone()),
            data: obj(json!({"from": from, "to": url, "nav_kind": nav_kind, "title": title})),
        },
        DomMutation { target, op, added, removed, text_delta, role, name, attr, .. } => Event {
            t,
            kind: "dom_mutation".into(),
            text: name.clone(),
            data: obj(json!({"target": target, "op": op, "added": added, "removed": removed, "text_delta": text_delta, "role": role, "attr": attr})),
        },
        Console { level, text, stack, source, uncaught, .. } => {
            let kind = if *uncaught || level == "error" || level == "assert" {
                "console_error"
            } else if level == "warn" || level == "warning" {
                "console_warn"
            } else {
                "console_log"
            };
            Event {
                t,
                kind: kind.into(),
                text: Some(text.clone()),
                data: obj(json!({"level": level, "source": source, "stack": stack, "uncaught": uncaught})),
            }
        }
        Network { method, url, status, status_text, resource_type, duration_ms, error_text, request_id, initiator, .. } => {
            let is_err = network_is_error(*status, error_text);
            let st = status.map(|s| s.to_string()).unwrap_or_else(|| "-".into());
            Event {
                t,
                kind: "network".into(),
                text: Some(format!("{method} {} {st}", path_of(url))),
                // strip the query string: tokens/secrets commonly ride in `?...`
                data: obj(json!({"method": method, "url": strip_query(url), "status": status, "status_text": status_text,
                    "resource_type": resource_type, "duration_ms": duration_ms, "is_error": is_err,
                    "error_text": error_text, "request_id": request_id, "initiator": initiator})),
            }
        }
        Click { click_kind, target, label, x, y, .. } => {
            let kind = if click_kind == "contextmenu" { "context_menu" } else { "click" };
            Event {
                t,
                kind: kind.into(),
                text: label.clone(),
                data: obj(json!({"click_kind": click_kind, "target": target, "x": x, "y": y})),
            }
        }
        Input { target, label, value, checked, masked, submit, .. } => {
            let kind = if *submit { "form_submit" } else { "input" };
            // never echo a masked value into data
            let v = if *masked { json!(null) } else { json!(value) };
            Event {
                t,
                kind: kind.into(),
                text: label.clone(),
                data: obj(json!({"target": target, "value": v, "checked": checked, "masked": masked})),
            }
        }
        Scroll { target, x, y, .. } => Event {
            t,
            kind: "scroll".into(),
            text: None,
            data: obj(json!({"target": target, "x": x, "y": y})),
        },
        A11yText { .. } | Screenshot { .. } | DomSnapshot { .. } | Unknown => return None,
    };
    Some(e)
}

/// §2.2 — DOM-verbatim on_screen_text (`source:"dom"`, `bbox:null`).
fn push_on_screen_text(idx: &mut Index, ev: &TraceEvent, t: f64, end: f64) {
    let add = |idx: &mut Index, start: f64, e: f64, text: String| {
        if idx.on_screen_text.len() >= MAX_ON_SCREEN_TEXT {
            return;
        }
        let text = cap_chars(text.trim(), MAX_TEXT_CHARS);
        if !text.is_empty() {
            idx.on_screen_text.push(OnScreenText { start, end: e, text, source: TextKind::Dom, bbox: None });
        }
    };
    match ev {
        TraceEvent::DomSnapshot { text: Some(txt), .. } => {
            for line in txt.lines().take(MAX_SNAPSHOT_LINES) {
                add(idx, t, end, line.to_string());
            }
        }
        TraceEvent::A11yText { text, sensitive, valid_until_ms, .. } if !sensitive => {
            let e = valid_until_ms.map(|v| v as f64 / 1000.0).unwrap_or(end);
            add(idx, t, e, text.clone());
        }
        TraceEvent::DomMutation { name: Some(nm), .. } => add(idx, t, end, nm.clone()),
        TraceEvent::Console { text, .. } => add(idx, t, t + EPS_S, text.clone()),
        TraceEvent::Network { method, url, status, status_text, error_text, .. }
            if network_is_error(*status, error_text) =>
        {
            let st = status.map(|s| s.to_string()).unwrap_or_else(|| error_text.clone().unwrap_or_default());
            let stx = status_text.as_deref().map(|x| format!(" {x}")).unwrap_or_default();
            add(idx, t, t + EPS_S, format!("{method} {} {st}{stx}", path_of(url)));
        }
        TraceEvent::Input { value, masked, .. } if !masked => add(idx, t, end, value.clone()),
        _ => {}
    }
}

/// §2.4 — record a redaction marker for masked/sensitive fields (value never copied).
fn push_redaction(out: &mut Vec<RedactionItem>, ev: &TraceEvent, t: f64) {
    let item = match ev {
        TraceEvent::Input { masked: true, label, .. } => Some((label.clone().unwrap_or_else(|| "input".into()),)),
        TraceEvent::A11yText { sensitive: true, role, .. } => Some((role.clone().unwrap_or_else(|| "field".into()),)),
        _ => None,
    };
    if let Some((entity,)) = item {
        out.push(RedactionItem { stream: "on_screen_text".into(), t, entity, action: "masked".into() });
    }
}

/// Attach the nearest screenshot (within 0.7s) to each salient moment as `frame_ref`.
fn attach_frames(idx: &mut Index, trace: &BrowserTrace) {
    let shots: Vec<(f64, String)> = trace
        .events
        .iter()
        .filter_map(|e| match e {
            TraceEvent::Screenshot { t_ms, path, .. } => Some((trace.rel_s(*t_ms), path.clone())),
            _ => None,
        })
        .collect();
    if shots.is_empty() {
        return;
    }
    for m in &mut idx.visual_timeline {
        if let Some((st, p)) = shots
            .iter()
            .min_by(|a, b| (a.0 - m.t).abs().partial_cmp(&(b.0 - m.t).abs()).unwrap_or(std::cmp::Ordering::Equal))
        {
            if (st - m.t).abs() <= 0.7 {
                m.frame_ref = Some(p.clone());
            }
        }
    }
}

/// §3.9 — tldr from the top-salience moment; chapters bounded by navigations, each titled
/// by the highest-salience moment in its span.
fn derive_summary(idx: &Index) -> Summary {
    let mut moments: Vec<&VisualMoment> = idx.visual_timeline.iter().collect();
    moments.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));

    let top = moments
        .iter()
        .max_by(|a, b| a.salience.partial_cmp(&b.salience).unwrap_or(std::cmp::Ordering::Equal));
    let tldr = top.map(|m| m.caption.clone()).unwrap_or_else(|| {
        format!("Browser session on {}", idx.metadata.url_context.as_deref().unwrap_or(""))
    });

    // navigation boundaries
    let navs: Vec<f64> = moments.iter().filter(|m| m.delta == "navigation").map(|m| m.t).collect();
    let bounds: Vec<f64> = if navs.is_empty() { vec![0.0] } else { navs };
    let mut chapters = Vec::new();
    for (k, &start) in bounds.iter().enumerate() {
        let next = bounds.get(k + 1).copied().unwrap_or(f64::INFINITY);
        let title = moments
            .iter()
            .filter(|m| m.t >= start && m.t < next)
            .max_by(|a, b| a.salience.partial_cmp(&b.salience).unwrap_or(std::cmp::Ordering::Equal))
            .map(|m| truncate(&m.caption, 70))
            .unwrap_or_else(|| "Session".into());
        chapters.push(Chapter { start, title });
    }
    Summary { tldr, chapters }
}

fn first_url(trace: &BrowserTrace) -> String {
    if !trace.url.is_empty() {
        return trace.url.clone();
    }
    trace
        .events
        .iter()
        .find_map(|e| match e {
            TraceEvent::Navigate { url, .. } => Some(url.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn page_title(trace: &BrowserTrace) -> Option<String> {
    trace.events.iter().find_map(|e| match e {
        TraceEvent::Navigate { title: Some(t), .. } if !t.is_empty() => Some(t.clone()),
        _ => None,
    })
}

fn obj(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match v {
        serde_json::Value::Object(m) => m,
        _ => serde_json::Map::new(),
    }
}

fn path_of(url: &str) -> String {
    let rest = match url.find("://") {
        Some(i) => &url[i + 3..],
        None => return url.split(['?', '#']).next().unwrap_or(url).to_string(),
    };
    match rest.find('/') {
        Some(j) => rest[j..].split(['?', '#']).next().unwrap_or(&rest[j..]).to_string(),
        None => "/".into(),
    }
}

fn truncate(s: &str, n: usize) -> String {
    cap_chars(s, n)
}

fn cap_chars(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n).collect();
        t.push('…');
        t
    }
}

/// Drop the `?query`/`#fragment` from a URL — tokens and secrets commonly ride there.
fn strip_query(url: &str) -> String {
    url.split(['?', '#']).next().unwrap_or(url).to_string()
}

/// Truncate a stream to `max`, logging (never silently dropping) what was cut.
fn cap_stream<T>(v: &mut Vec<T>, max: usize, name: &str) {
    if v.len() > max {
        tracing::warn!(stream = name, len = v.len(), cap = max, "trace stream exceeded cap; truncating");
        v.truncate(max);
    }
}
