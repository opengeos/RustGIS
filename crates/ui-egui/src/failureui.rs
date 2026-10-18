//! A failed fetch told in two levels (#509): what happened and what to do next in plain words,
//! and, folded under Details, what the server said, for an expert or the service's administrator.
//!
//! What the server said is untrusted: shown as plain text only, its address without keys.

use egui::{RichText, Ui, vec2};
use rustgis_engine::render::rustgis_tiles::failure::{Failure, Reason, status_name};

use crate::theme::Tokens;

/// What happened, and what to do next (translated).
pub fn plain(f: &Failure) -> (String, String) {
    let refused = tl!("The server refused the request.");
    match (f.reason, f.firewall()) {
        (Reason::Status(401), _) => (refused.into(), tl!("The service needs a sign-in or a token.").into()),
        (Reason::Status(403), Some(firewall)) => (
            refused.into(),
            crate::i18n::fmt(
                "Its firewall ({firewall}) blocked RustGIS. The service may only allow some applications: contact its administrator and send them the details below.",
                &[("firewall", firewall)],
            ),
        ),
        (Reason::Status(403), None) => (refused.into(), tl!("The service may be private or need a token.").into()),
        (Reason::Status(404), _) => (tl!("Nothing was found at this address.").into(), tl!("Check the address.").into()),
        (Reason::Status(429), _) => (tl!("The server is limiting requests.").into(), tl!("Try again in a moment.").into()),
        (Reason::Status(s), _) if s >= 500 => (tl!("The server had a problem answering.").into(), tl!("Try again later.").into()),
        (Reason::Status(_), _) => (refused.into(), tl!("Check the address and the service type.").into()),
        (Reason::Unreachable, _) => {
            (tl!("RustGIS couldn't reach the server.").into(), tl!("Check the address and your connection.").into())
        }
        (Reason::Timeout, _) => (tl!("The server took too long to answer.").into(), tl!("Try again later.").into()),
        (Reason::Other, _) => (tl!("The request failed.").into(), f.message.clone()),
    }
}

/// What is known of the failure, as (label, value) rows: the labels in English, translated
/// where shown.
pub fn details(f: &Failure) -> Vec<(&'static str, String)> {
    let mut rows = vec![("Address", rustgis_engine::sources::redact(&f.url))];
    match f.status() {
        Some(s) => rows.push(("Status", status_name(s).map_or_else(|| s.to_string(), |n| format!("{s} {n}")))),
        None => rows.push(("Error", f.message.clone())),
    }
    let header = |k: &str| f.header(k).map(str::to_string);
    for (label, value) in [
        ("Server", header("server")),
        ("Firewall", f.firewall().map(str::to_string)),
        ("Request ID", f.request_id()),
        ("Sign-in", header("www-authenticate")),
        ("Retry after", header("retry-after")),
        ("Sent as", f.user_agent.as_ref().map(|u| format!("User-Agent: {u}"))),
        ("Response", f.excerpt.as_ref().map(|e| format!("\"{e}\""))),
    ] {
        if let Some(v) = value {
            rows.push((label, v));
        }
    }
    rows
}

/// The details as text to send to the service's administrator (in English, like the server's
/// own words).
pub fn report(f: &Failure) -> String {
    details(f).iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n")
}

/// The failure: its plain message, always shown, and its details, folded.
pub fn show(ui: &mut Ui, f: &Failure, t: &Tokens) {
    let (what, next) = plain(f);
    ui.label(RichText::new(what).strong().color(t.red));
    ui.label(next);
    egui::CollapsingHeader::new(tl!("Details")).id_salt("fetch_failure_details").default_open(false).show(ui, |ui| {
        egui::Grid::new("fetch_failure_rows").num_columns(2).spacing(vec2(12.0, 4.0)).show(ui, |ui| {
            for (label, value) in details(f) {
                ui.label(RichText::new(tl!(label)).color(t.text_dim));
                ui.add(egui::Label::new(RichText::new(value).monospace().size(11.5)).wrap());
                ui.end_row();
            }
        });
        ui.add_space(4.0);
        if ui.button(tl!("Copy details")).clicked() {
            ui.ctx().copy_text(report(f));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_firewall_block_is_told_plainly_with_its_details() {
        let headers = [("server".to_string(), "cloudflare".to_string()), ("cf-ray".to_string(), "a4c9-BOG".to_string())];
        let mut f = Failure::answered("https://example.org/MapServer?f=json&token=secret", 403, &headers, b"<title>Blocked</title>");
        f.user_agent = Some("RustGIS/1.0".into());
        let (what, next) = plain(&f);
        assert_eq!(what, "The server refused the request.");
        assert!(next.contains("Cloudflare") && next.contains("administrator"), "{next}");
        let report = report(&f);
        assert!(!report.contains("secret"), "keys are left out: {report}");
        for line in [
            "Status: 403 Forbidden",
            "Server: cloudflare",
            "Firewall: Cloudflare",
            "Request ID: cf-ray a4c9-BOG",
            "Sent as: User-Agent: RustGIS/1.0",
            "Response: \"Blocked\"",
        ] {
            assert!(report.contains(line), "{line} in {report}");
        }
    }

    #[test]
    fn each_kind_of_failure_says_what_to_do() {
        let status = |s: u16| plain(&Failure::answered("u", s, &[], b"")).1;
        assert!(status(401).contains("sign-in"));
        assert!(status(403).contains("private"));
        assert!(status(404).contains("address"));
        assert!(status(429).contains("moment"));
        assert!(status(502).contains("later"));
        assert!(status(418).contains("service type"));
        let unanswered = |r: Reason| plain(&Failure::without_answer("u", "x", r)).0;
        assert!(unanswered(Reason::Unreachable).contains("couldn't reach"));
        assert!(unanswered(Reason::Timeout).contains("too long"));
        // Anything else: the error itself, with no status to show.
        let other = Failure::new("u", "too many redirects");
        assert_eq!(plain(&other).1, "too many redirects");
        assert!(report(&other).contains("Error: too many redirects"));
    }
}
