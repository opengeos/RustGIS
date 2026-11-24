//! What the addresses a file names may reach, by where the file came from.
//!
//! A project, map file, layer file, template or package the user opens from their own disk is
//! theirs: what it names reaches as far as the user's own addresses do. One that arrived from the
//! internet (opened by its web address, a GeoLibre project downloaded, or file text the Assistant,
//! MCP or a plugin supplied, which says nothing trustworthy about where it was read) is a document
//! read from the internet, and what it names reaches only the internet, as what any other such
//! document names does ([`cache::restrict_to_public`]). Either way, the addresses it names on this
//! computer or the local network are said as it opens.

use crate::Session;
use crate::render::rustgis_tiles::{address, address::Reach, cache, logins};
use serde_json::Value;

/// How many addresses a notice names; the count covers them all.
const NAMED: usize = 3;

impl Session {
    /// The widest reach what a file being read names may have (`p`: the command's parameters).
    /// A file downloaded by its web address is read within the reach it was read at
    /// ([`Session::receiving`]); file text or data supplied by anything but the user (the
    /// Assistant, MCP, a plugin) within the internet; a file read from a path, or given by the user,
    /// within this computer and the local network, as before.
    pub(crate) fn file_reach(&self, p: &Value) -> Reach {
        let supplied = ["text", "data"].iter().any(|k| p.get(k).is_some_and(Value::is_string));
        let given = if supplied && !self.policy_user() { Reach::Public } else { Reach::Local };
        self.received.map_or(given, |read| given.min(read.min(Reach::Local)))
    }

    /// Reads the files the commands run next open as downloaded from an address reached at
    /// `reach` (the app's open by web address: `Reach::Public` when the download came from the
    /// internet), until it is called again with `None`. Returns what it replaced, for the caller
    /// to put back.
    pub fn receiving(&mut self, reach: Option<Reach>) -> Option<Reach> {
        std::mem::replace(&mut self.received, reach)
    }
}

/// What a file names on this computer or the local network, gathered as it is read
/// ([`hold`]), for [`Named::say`].
#[derive(Debug, Default)]
#[must_use = "say what the file names"]
pub(crate) struct Named {
    /// The distinct addresses named there.
    local: Vec<String>,
    /// Whether they are held to the internet (the file arrived from there).
    held: bool,
    /// Whether holding them filled the record of addresses held there, so every address is.
    filled: bool,
}

/// Holds every web address a file names (`addresses`, its whole list as login bindings are cleared
/// from it) within `reach`, before anything it names is read: within the internet, each is
/// recorded so ([`cache::restrict_to_public`]), with a Shapefile's parts, so it is fetched only
/// from there, however its name resolves. Within the local network, nothing is recorded, and the
/// addresses reach as before. Returns what the file names on this computer or the local network.
pub(crate) fn hold(addresses: impl IntoIterator<Item = impl AsRef<str>>, reach: Reach) -> Named {
    let held = reach == Reach::Public;
    let full = cache::public_restrictions_full();
    let mut named = Named { held, ..Named::default() };
    for a in addresses {
        let a = a.as_ref();
        if !rustgis_map::tiles::is_web(a) {
            continue;
        }
        if held {
            cache::restrict_to_public(a);
            for (_, part) in rustgis_data::io::shapefile_part_urls(a).unwrap_or_default() {
                cache::restrict_to_public(&part);
            }
        }
        if address::reach(logins::inner(a)) != Reach::Public && !named.local.iter().any(|n| n == a) {
            named.local.push(a.to_string());
        }
    }
    named.filled = held && !full && cache::public_restrictions_full();
    named
}

impl Named {
    /// Says, once, what `what` (a project, a map file, a layer file) names on this computer or the
    /// local network, with how many and the first few: held to the internet, or read from there as
    /// the user's own. And that holding them filled the record of held addresses, if it did.
    pub(crate) fn say(self, s: &mut Session, what: &str) {
        if !self.local.is_empty() {
            let n = self.local.len();
            let mut shown: Vec<String> = self.local.iter().take(NAMED).map(|a| crate::sources::address_in_message(a)).collect();
            if n > NAMED {
                shown.push(format!("and {} more", n - NAMED));
            }
            let shown = shown.join(", ");
            s.notify(
                if self.held {
                    format!(
                        "{what} came from the internet, so the {n} address(es) it names on this computer or the local network are held to the internet and aren't read: {shown}"
                    )
                } else {
                    format!("{what} reads from {n} address(es) on this computer or the local network: {shown}")
                },
                false,
            );
        }
        if self.filled {
            s.notify(
                format!(
                    "holding what {what} names to the internet filled the record of addresses held there: until RustGIS is restarted, every address is held to the internet, and services on this computer or the local network can't be reached"
                ),
                false,
            );
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "a test module gated on more than `test`, which clippy's allow-unwrap-in-tests does not see"
)]
mod tests {
    use super::*;
    use crate::Project;
    use crate::policy::{AgentAccess, Call, Effects, Outcome, Revocation, Scope};
    use serde_json::json;

    /// Authority that is never revoked, for running a command as an agent would.
    struct Live;
    impl Revocation for Live {
        fn allows(&self, _: Effects) -> bool {
            true
        }
    }

    /// Runs `id` as an attached agent (MCP) with full access does, approved. (The Assistant
    /// refuses to run these documents at all.)
    fn as_agent(s: &mut Session, id: &str, p: &Value) -> crate::CmdResult {
        let scope = Scope::agent(AgentAccess::Full, false, "mcp", None, std::sync::Arc::new(Live));
        let call = Call::command(id, p.clone());
        match s.review(&scope, &call) {
            Outcome::NeedsApproval(permit) => s.run_permitted(scope, permit.allow(), &call),
            Outcome::Allowed => s.run_scoped(scope, &call),
            Outcome::Refused(e) => panic!("{id} refused: {e}"),
        }
    }

    /// A project naming a tile server on this computer, a layer's source on the local network and
    /// a tile server on the internet (hosts unique to `tag`, so tests running together don't share
    /// a record), with those three addresses.
    fn project(tag: &str) -> (Session, [String; 3]) {
        let local = format!("http://localhost:18080/{tag}/{{z}}/{{x}}/{{y}}.png");
        let lan = format!("http://192.168.1.10/{tag}/roads.fgb");
        let public = format!("https://{tag}.example/tiles/{{z}}/{{x}}/{{y}}.png");
        let mut s = Session::new(Project::default());
        s.project.name = tag.into();
        s.run("map.addTiles", &json!({"url": local, "name": "Mine"})).unwrap();
        s.run("map.addTiles", &json!({"url": public, "name": "Theirs"})).unwrap();
        let roads = r#"{"type":"FeatureCollection","features":[{"type":"Feature","properties":{"n":1},"geometry":{"type":"Point","coordinates":[8.5,47.4]}}]}"#;
        s.run("layer.addDataText", &json!({"fileName": "roads.geojson", "text": roads})).unwrap();
        let id = s.map().layers.last().map(|l| l.id).unwrap();
        s.map_mut().layer_mut(id).unwrap().dataset.source = Some(lan.clone());
        (s, [local, lan, public])
    }

    /// The reach each address has now, after the record of them is put back as a new process has
    /// it (the user's own: [`cache::allow_named`]) before `open` runs.
    fn reach_after(addresses: &[String; 3], open: impl FnOnce()) -> [Reach; 3] {
        for a in addresses {
            cache::allow_named(a);
        }
        open();
        [0, 1, 2].map(|i| cache::ceiling_for(&addresses[i]))
    }

    /// The notices that say what a file names on this computer or the local network.
    fn said(s: &Session) -> Vec<String> {
        s.notices.iter().filter(|n| n.text.contains("on this computer or the local network")).map(|n| n.text.clone()).collect()
    }

    /// A project opened from the user's own disk, or given by the user, reads its servers on this
    /// computer and the local network as before, and says so once, with how many and which. One
    /// that names none says nothing.
    #[test]
    fn a_project_from_your_own_disk_keeps_its_local_servers_and_names_them() {
        let (mut s, addresses) = project("own-disk-reach");
        let (text, _) = s.project.saved_text(None).unwrap();
        let dir = std::env::temp_dir().join(format!("rustgis-own-disk-reach-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("own.rgis");
        std::fs::write(&path, &text).unwrap();
        let mut opened = Session::new(Project::default());
        let reach = reach_after(&addresses, || {
            opened.run("project.open", &json!({"path": path.display().to_string()})).unwrap();
        });
        assert_eq!(reach, [Reach::Local; 3], "opened from disk");
        let notices = said(&opened);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].starts_with("the project 'own' reads from 2 address(es)"), "{notices:?}");
        assert!(notices[0].contains("localhost:18080") && notices[0].contains("192.168.1.10"), "{notices:?}");
        let mut given = Session::new(Project::default());
        let reach = reach_after(&addresses, || {
            given.run("project.openText", &json!({"text": text})).unwrap();
        });
        assert_eq!(reach, [Reach::Local; 3], "given by the user");
        assert_eq!(said(&given).len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
        // Nothing on this computer or the local network: nothing said.
        let mut plain = Session::new(Project::default());
        plain.run("map.addTiles", &json!({"url": "https://own-disk-plain.example/{z}/{x}/{y}.png", "name": "Theirs"})).unwrap();
        let (text, _) = plain.project.saved_text(None).unwrap();
        let mut opened = Session::new(Project::default());
        opened.run("project.openText", &json!({"text": text})).unwrap();
        assert_eq!(said(&opened), Vec::<String>::new());
    }

    /// A project received from the internet (downloaded from there, or given as text by an
    /// attached agent) reaches only the internet: every address it names is held there, before any is
    /// read, and the ones it names on this computer or the local network are said, once. One
    /// downloaded from a server on the local network reads from there as the user's own.
    #[test]
    fn a_project_received_from_the_internet_reaches_only_the_internet() {
        let (mut s, addresses) = project("received-reach");
        let (text, _) = s.project.saved_text(None).unwrap();
        let mut downloaded = Session::new(Project::default());
        let reach = reach_after(&addresses, || {
            let was = downloaded.receiving(Some(Reach::Public));
            downloaded.run("project.openText", &json!({"text": text, "fileName": "shared.rgis"})).unwrap();
            downloaded.receiving(was);
        });
        assert_eq!(reach, [Reach::Public; 3], "downloaded from the internet");
        let notices = said(&downloaded);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].starts_with("the project 'shared' came from the internet, so the 2 address(es)"), "{notices:?}");
        assert!(notices[0].contains("held to the internet"), "{notices:?}");
        assert_eq!(downloaded.received, None, "put back");
        let mut supplied = Session::new(Project::default());
        let reach = reach_after(&addresses, || {
            as_agent(&mut supplied, "project.openText", &json!({"text": text})).unwrap();
        });
        assert_eq!(reach, [Reach::Public; 3], "given by an agent");
        let mut from_lan = Session::new(Project::default());
        let reach = reach_after(&addresses, || {
            from_lan.receiving(Some(Reach::Local));
            from_lan.run("project.openText", &json!({"text": text})).unwrap();
        });
        assert_eq!(reach, [Reach::Local; 3], "downloaded from the local network");
        assert!(said(&from_lan)[0].contains("reads from 2 address(es)"));
    }

    /// A map file, a layer file and a template given as text by an attached agent hold what they
    /// name to the internet too; the same files given by the user don't.
    #[test]
    fn map_layer_and_template_files_an_agent_gives_reach_only_the_internet() {
        let (mut s, addresses) = project("item-reach");
        let (template, _) = s.project.saved_text(None).unwrap();
        let map = String::from_utf8(crate::project::map_file_bytes(s.map()).unwrap()).unwrap();
        let layer = String::from_utf8(crate::project::layer_file_bytes(s.map().layers.last().unwrap()).unwrap()).unwrap();
        for (id, text, held) in [
            ("map.import", &map, [true, true, true]),
            ("layer.addLayerFile", &layer, [false, true, false]),
            ("project.fromTemplate", &template, [true, true, true]),
        ] {
            let p = json!({"text": text});
            let mut by_user = Session::new(Project::default());
            assert_eq!(reach_after(&addresses, || drop(by_user.run(id, &p).unwrap())), [Reach::Local; 3], "{id} by the user");
            assert_eq!(said(&by_user).len(), 1, "{id}");
            let mut supplied = Session::new(Project::default());
            let reach = reach_after(&addresses, || drop(as_agent(&mut supplied, id, &p).unwrap()));
            assert_eq!(reach, held.map(|h| if h { Reach::Public } else { Reach::Local }), "{id} by an agent");
            let notices = said(&supplied);
            assert_eq!(notices.len(), 1, "{id}: {notices:?}");
            assert!(notices[0].contains("came from the internet"), "{id}: {notices:?}");
        }
    }

    /// A GeoLibre project an attached agent gives holds its background and layers to the internet
    /// before any is read; the same project given by the user doesn't.
    #[test]
    fn a_geolibre_project_an_agent_gives_reaches_only_the_internet() {
        let tiles = "http://localhost:18082/geolibre-reach/{z}/{x}/{y}.png".to_string();
        let style = "http://localhost:18083/geolibre-reach/style.json".to_string();
        let public = "https://geolibre-reach.example/{z}/{x}/{y}.png".to_string();
        let text = json!({
            "version": "0.2.0",
            "name": "Received",
            "mapView": {},
            "basemapStyleUrl": style,
            "layers": [
                {"id": "a", "name": "Mine", "type": "xyz", "source": {"tiles": [tiles]}},
                {"id": "b", "name": "Theirs", "type": "xyz", "source": {"tiles": [public]}},
            ],
        })
        .to_string();
        let addresses = [tiles, style, public];
        let p = json!({"text": text});
        let mut by_user = Session::new(Project::default());
        assert_eq!(reach_after(&addresses, || drop(by_user.run("project.importGeolibre", &p))), [Reach::Local; 3]);
        assert!(said(&by_user).first().is_some_and(|n| n.contains("reads from 2 address(es)")), "{:?}", said(&by_user));
        let mut supplied = Session::new(Project::default());
        let reach = reach_after(&addresses, || drop(as_agent(&mut supplied, "project.importGeolibre", &p)));
        assert_eq!(reach, [Reach::Public; 3]);
        let notices = said(&supplied);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].starts_with("the GeoLibre project 'Received' came from the internet, so the 2 address(es)"), "{notices:?}");
    }

    /// A GeoLibre project downloaded from the internet whose layer the app downloads later (as
    /// the desktop app and the browser do) asks for it as an address a document read from there
    /// named, so adding it when it lands keeps the hold; the user's own asks as before.
    #[test]
    fn a_received_geolibre_layer_downloaded_later_stays_held() {
        let data = "https://geolibre-later.example/roads.geojson".to_string();
        let text = json!({
            "version": "0.2.0",
            "name": "Later",
            "mapView": {},
            "basemapStyleUrl": "",
            "layers": [{"id": "a", "name": "Roads", "type": "geojson", "source": {"url": data}}],
        })
        .to_string();
        let p = json!({"text": text});
        for received in [Some(Reach::Public), None] {
            let mut s = Session::new(Project::default());
            let reach = reach_after(&[data.clone(), data.clone(), data.clone()], || {
                s.receiving(received);
                s.run_leaving_downloads("project.importGeolibre", &p).unwrap();
                s.receiving(None);
            });
            let asked: Vec<&Value> = s.url_requests.iter().map(|r| r.params()).collect();
            assert_eq!(asked.len(), 1, "{asked:?}");
            let from_document = asked[0].get("fromDocument").and_then(Value::as_bool) == Some(true);
            assert_eq!((reach[0], from_document), if received.is_some() { (Reach::Public, true) } else { (Reach::Local, false) });
        }
    }
}
