//! `/about`, rules, terms of service and privacy policy, and the admin
//! settings that change them.

use super::*;

#[test]
fn default_rules_and_generated_terms() {
    let mut s = Server::provisioned();
    let rules = s.get("/api/v1/instance/rules", None).json_body();
    assert_eq!(rules.as_array().unwrap().len(), 3);
    assert_eq!(rules[0]["id"], "1");
    assert_eq!(rules[1]["text"], "Agree to not unplug the microcontroller");

    let terms = s.get("/api/v1/instance/terms_of_service", None).json_body();
    assert_eq!(terms["effective"], true);
    let content = terms["content"].as_str().unwrap();
    assert!(content.contains("Home is a private server"), "{content}");
    assert!(content.contains("1. Agree to be patient"));
    let day = terms["effective_date"].as_str().unwrap().to_string();
    let url = format!("/api/v1/instance/terms_of_service/{day}");
    assert_eq!(s.get(&url, None).status, 200);
    assert_eq!(
        s.get("/api/v1/instance/terms_of_service/1999-01-01", None)
            .status,
        404
    );

    let privacy = s.get("/api/v1/instance/privacy_policy", None).json_body();
    assert!(privacy["content"].as_str().unwrap().contains("encrypted"));
    let v2 = s.get("/api/v2/instance", None).json_body();
    assert_eq!(
        v2["configuration"]["urls"]["terms_of_service"],
        "http://mastomini.test/terms-of-service"
    );
    assert_eq!(
        v2["configuration"]["urls"]["about"],
        "http://mastomini.test/about"
    );
}

#[test]
fn about_pages_render_without_signing_in() {
    let mut s = Server::provisioned();
    let about = s.get("/about", None);
    assert_eq!(about.status, 200);
    let page = html(&about);
    assert!(page.contains("<h1>Home</h1>"));
    assert!(page.contains("<li>Agree to be patient, the microcontroller is slow</li>"));
    assert!(page.contains("@alice (owner)"));
    assert!(html(&s.get("/terms-of-service", None)).contains("Effective "));
    assert!(html(&s.get("/privacy-policy", None)).contains("Direct messages are encrypted"));
}

#[test]
fn admins_edit_rules_and_terms() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    let bob = s.add_member(&alice, "bob");
    let url = "/api/mastomini/v1/admin/server";
    let body = json!({ "rules": ["Be kind", "Feed the cat"], "terms": "House rules apply.", "description": "Our place" });
    assert_eq!(s.send_json("PUT", url, Some(&bob), &body).status, 403);
    let r = s.send_json("PUT", url, Some(&alice), &body);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json_body()["terms_customized"], true);

    let mut s = s.restart();
    let rules = s.get("/api/v1/instance/rules", None).json_body();
    assert_eq!(rules[1]["text"], "Feed the cat");
    let terms = s.get("/api/v1/instance/terms_of_service", None).json_body();
    assert_eq!(terms["content"], "<p>House rules apply.</p>");
    let about = s
        .get("/api/v1/instance/extended_description", None)
        .json_body();
    assert_eq!(about["content"], "<p>Our place</p>");

    // An empty terms text goes back to the generated terms.
    let r = s.send_json("PUT", url, Some(&alice), &json!({ "terms": "" }));
    assert_eq!(r.json_body()["terms_customized"], false);
    let too_many: Vec<String> = (0..9).map(|i| format!("rule {i}")).collect();
    let r = s.send_json("PUT", url, Some(&alice), &json!({ "rules": too_many }));
    assert_eq!(r.status, 422);
}
