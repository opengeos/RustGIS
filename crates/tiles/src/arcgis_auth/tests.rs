use super::*;

// The sessions are shared by the process's tests: each test uses its own hosts.

fn grant(token: &str, expires_at: i64) -> Grant {
    Grant { access_token: token.into(), refresh_token: Some("refresh".into()), expires_at, username: "ana".into() }
}

#[test]
fn portal_bases() {
    assert_eq!(portal_base("").unwrap(), ONLINE);
    assert_eq!(portal_base("https://arcgis.com/home").unwrap(), ONLINE);
    assert_eq!(portal_base("https://MyOrg.maps.arcgis.com/home/content.html").unwrap(), "https://myorg.maps.arcgis.com");
    assert_eq!(portal_base("https://gis.example.org/portal/sharing/rest?f=json").unwrap(), "https://gis.example.org/portal");
    assert_eq!(portal_base("https://gis.example.org:7443/arcgis/home/").unwrap(), "https://gis.example.org:7443/arcgis");
    assert_eq!(portal_base("https://gis.example.org:443/portal").unwrap(), "https://gis.example.org/portal");
    // Tokens travel to the portal: never over plain HTTP, never with user info.
    for bad in ["http://gis.example.org/portal", "https://user@gis.example.org/portal", "https://", "https://gis.example.org:x/", "ftp://a"]
    {
        assert!(portal_base(bad).is_err(), "{bad}");
    }
    assert!(is_online("https://myorg.maps.arcgis.com") && is_online(ONLINE) && !is_online("https://gis.example.org/portal"));
}

#[test]
fn endpoints_and_requests() {
    // An organization's users sign in on its page; its tokens come from www.arcgis.com.
    assert_eq!(endpoint("https://myorg.maps.arcgis.com", "authorize"), "https://myorg.maps.arcgis.com/sharing/rest/oauth2/authorize");
    assert_eq!(endpoint("https://myorg.maps.arcgis.com", "token"), "https://www.arcgis.com/sharing/rest/oauth2/token");
    assert_eq!(endpoint("https://gis.example.org/portal", "token"), "https://gis.example.org/portal/sharing/rest/oauth2/token");
    let url = authorize_url("https://gis.example.org/portal", "abc123", &redirect_uri(PORT), "chal", "st&te");
    assert!(url.starts_with("https://gis.example.org/portal/sharing/rest/oauth2/authorize?client_id=abc123&response_type=code"));
    assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A7290%2Farcgis%2Fcallback"));
    assert!(url.contains("code_challenge=chal&code_challenge_method=S256&state=st%26te"));
    let body = exchange_body("abc", "c/d", "http://127.0.0.1:7290/arcgis/callback", "v");
    assert!(body.contains("grant_type=authorization_code") && body.contains("code=c%2Fd") && body.contains("code_verifier=v"));
    assert!(refresh_body("abc", "r").contains("grant_type=refresh_token&client_id=abc&refresh_token=r"));
    assert!(revoke_body("abc", "r", true).ends_with("auth_token=r&token_type_hint=refresh_token"));
    // RFC 7636's example.
    assert_eq!(pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    assert!(valid_client_id("3IKyMtbdeceo0JCr") && !valid_client_id("") && !valid_client_id("a b") && !valid_client_id("a&b"));
}

#[test]
fn token_answers() {
    let g = parse_grant(br#"{"access_token":"tok.1","refresh_token":"ref","expires_in":1800,"username":"ana"}"#, 100).unwrap();
    assert_eq!(
        (g.access_token.as_str(), g.refresh_token.as_deref(), g.expires_at, g.username.as_str()),
        ("tok.1", Some("ref"), 1900, "ana")
    );
    // Refusals come as HTTP 200 with an error object.
    let e = parse_grant(br#"{"error":{"code":400,"error":"invalid_request","error_description":"Invalid PKCE code_verifier"}}"#, 0)
        .unwrap_err();
    assert!(e.contains("Invalid PKCE code_verifier"), "{e}");
    assert!(parse_grant(b"<html>", 0).is_err());
    assert!(parse_grant(br#"{"access_token":"bad token\r\nX: y"}"#, 0).is_err());
    // No lifetime: half an hour.
    assert_eq!(parse_grant(br#"{"access_token":"t"}"#, 0).unwrap().expires_at, 1800);
    // Its Debug form hides the tokens.
    assert!(!format!("{g:?}").contains("tok.1"));
}

#[test]
fn an_online_sign_in_covers_its_organization_only() {
    let portal = "https://scopeorg.maps.arcgis.com";
    sign_in(portal, "client", grant("online-token", now_secs() + 3600), scopes_for(portal, Some("Org1d"), &[]));
    let bearer = Some("Bearer online-token".to_string());
    assert_eq!(authorization("https://scopeorg.maps.arcgis.com/sharing/rest/content/items/x?f=json"), bearer);
    assert_eq!(authorization("https://services1.arcgis.com/Org1d/arcgis/rest/services/A/FeatureServer/0/query?f=json"), bearer);
    assert_eq!(authorization("https://tiles.arcgis.com/tiles/Org1d/arcgis/rest/services/B/MapServer/tile/1/0/0"), bearer);
    // Another organization's services, Esri's basemaps, look-alike hosts and plain HTTP don't get it.
    for url in [
        "https://services1.arcgis.com/0ther/arcgis/rest/services/A/FeatureServer",
        "https://services.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/1/0/0",
        "https://basemaps.arcgis.com/arcgis/rest/services/World_Basemap_v2/VectorTileServer",
        "https://services1.arcgis.com.evil.example/Org1d/arcgis/rest/services/A/FeatureServer",
        "https://evil.example/Org1d/arcgis/rest/services/A/FeatureServer",
        "http://services1.arcgis.com/Org1d/arcgis/rest/services/A/FeatureServer",
        "https://user@services1.arcgis.com/Org1d/arcgis/rest/services/A/FeatureServer",
        "https://services1.arcgis.com/x/y/Org1d/A/FeatureServer",
    ] {
        assert_eq!(authorization(url), None, "{url}");
    }
    assert!(signed_in(portal).is_some_and(|i| i.username == "ana" && i.renews && !i.token));
    let (revocations, gone) = sign_out(Some(portal));
    assert_eq!(gone, 1);
    assert!(revocations.len() == 1 && revocations[0].endpoint == "https://www.arcgis.com/sharing/rest/oauth2/revokeToken");
    assert_eq!(authorization("https://services1.arcgis.com/Org1d/arcgis/rest/services/A/FeatureServer"), None);
}

#[test]
fn an_enterprise_sign_in_covers_its_host_and_federated_servers() {
    let portal = "https://ent.example.org/portal";
    let servers = parse_servers(br#"{"servers":[{"url":"https://ent.example.org/server"},{"url":"https://maps.ent-other.example.org/arcgis"},{"url":"http://plain.example.org/server"}]}"#);
    assert_eq!(servers, ["https://ent.example.org/server", "https://maps.ent-other.example.org/arcgis"]);
    sign_in(portal, "client", grant("ent-token", now_secs() + 3600), scopes_for(portal, None, &servers));
    let bearer = Some("Bearer ent-token".to_string());
    assert_eq!(authorization("https://ent.example.org/server/rest/services/Roads/MapServer?f=json"), bearer);
    assert_eq!(authorization("https://maps.ent-other.example.org/ArcGIS/rest/services/A/FeatureServer"), bearer);
    assert_eq!(authorization("https://maps.ent-other.example.org/other/rest/services"), None);
    assert_eq!(authorization("https://ent.example.org.evil.example/server/rest/services"), None);
    // An expired token without renewal isn't sent.
    sign_in(portal, "client", grant("old", now_secs() - 1), scopes_for(portal, None, &[]));
    assert_eq!(authorization("https://ent.example.org/server/rest/services"), None);
    sign_out(Some(portal));
}

#[test]
fn a_token_in_a_service_address_reaches_its_layers() {
    // The reported case (#914): a service's address with ?token=… works in the browser, but its
    // layers, tiles and feature pages are fetched from addresses built without it.
    let url = "https://tokenserver.example.org/server/rest/services/Secure/MapServer?f=json&token=abc.DEF-123";
    let kept = adopt_url_token(url).unwrap();
    assert_eq!(kept, "https://tokenserver.example.org/server/rest/services/Secure/MapServer?f=json");
    let bearer = Some("Bearer abc.DEF-123".to_string());
    assert_eq!(authorization("https://tokenserver.example.org/server/rest/services/Secure/MapServer/tile/3/2/1"), bearer);
    assert_eq!(
        authorization("https://tokenserver.example.org/server/rest/services/Secure/MapServer/0/query?where=1%3D1&f=geojson"),
        bearer
    );
    assert_eq!(authorization("https://tokenserver.example.org/other/rest/services"), None);
    assert!(list().iter().any(|i| i.portal == "https://tokenserver.example.org/server" && i.token && i.expires_at.is_none()));
    // An address without a token is unchanged; a hostile one is refused.
    assert_eq!(
        adopt_url_token("https://a.example.org/server/rest/services?f=json").unwrap(),
        "https://a.example.org/server/rest/services?f=json"
    );
    assert!(adopt_url_token("https://b.example.org/server/rest/services?token=a%0D%0Ab").is_err());
    assert!(adopt_url_token("http://c.example.org/server/rest/services?token=abc").is_err());
    assert_eq!(
        adopt_url_token("https://d.example.org/server/rest/services/X/MapServer?token=").unwrap(),
        "https://d.example.org/server/rest/services/X/MapServer"
    );
    assert_eq!(
        server_root("https://services1.arcgis.com/Org/arcgis/rest/services/A/FeatureServer").as_deref(),
        Some("https://services1.arcgis.com/Org/arcgis/")
    );
    assert_eq!(server_root("https://e.example.org/rest/services").as_deref(), Some("https://e.example.org/"));
    assert_eq!(server_root("https://e.example.org/data.geojson"), None);
    sign_out(Some("https://tokenserver.example.org/server/"));
    assert_eq!(authorization("https://tokenserver.example.org/server/rest/services/Secure/MapServer/tile/3/2/1"), None);
}

#[test]
fn a_server_token_wins_over_a_portal_sign_in() {
    let portal = "https://both.example.org/portal";
    sign_in(portal, "client", grant("portal-token", now_secs() + 3600), scopes_for(portal, None, &[]));
    add_token("https://both.example.org/server/rest/services", "server-token").unwrap();
    assert_eq!(authorization("https://both.example.org/server/rest/services/A/MapServer").as_deref(), Some("Bearer server-token"));
    assert_eq!(authorization("https://both.example.org/portal/sharing/rest/portals/self").as_deref(), Some("Bearer portal-token"));
    sign_out(Some(portal));
    sign_out(Some("https://both.example.org/server"));
}

#[test]
fn hints_for_refusals() {
    assert!(refused_hint(499).is_some_and(|h| h.contains("arcgis.signIn")));
    assert!(refused_hint(498).is_some());
    assert!(refused_hint(400).is_none());
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_sign_in_refuses_bad_input_before_listening() {
    assert!(start_sign_in("http://gis.example.org/portal", "abc", 0).is_err());
    assert!(start_sign_in("", "", 0).unwrap_err().contains("client id"));
}
