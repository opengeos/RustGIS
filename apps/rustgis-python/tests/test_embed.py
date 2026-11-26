"""URL construction and validation for the embedded web app widgets."""

import shutil
import subprocess

import pytest

from rustgis._embed import DEFAULT_URL, WIDGET_JS, _app_src


def test_app_src_preserves_query_and_fragment(monkeypatch):
    monkeypatch.delenv("RUSTGIS_WEB_URL", raising=False)
    assert _app_src(None, None) == f"{DEFAULT_URL}?embed"
    assert _app_src("http://127.0.0.1:8772/", "de") == "http://127.0.0.1:8772/?embed&lang=de"
    assert _app_src("https://example.com:8443/app", None) == "https://example.com:8443/app?embed"
    assert _app_src("https://web.rustgis.app/?foo=1", "fr") == "https://web.rustgis.app/?foo=1&embed&lang=fr"
    assert _app_src("https://web.rustgis.app/#map", None) == "https://web.rustgis.app/?embed#map"
    assert _app_src("https://web.rustgis.app/?foo=1#map", "de") == "https://web.rustgis.app/?foo=1&embed&lang=de#map"


@pytest.mark.parametrize(
    "bad_url",
    [
        "javascript:alert(1)",
        "JAVASCRIPT:alert(1)",
        "data:text/html,<h1>x</h1>",
        "file:///etc/passwd",
        "https://",
        "http://",
        "https://:443",
        "/relative/path",
        "ftp://example.org/app",
    ],
)
def test_app_src_rejects_non_http_or_hostless_urls(bad_url):
    with pytest.raises(ValueError):
        _app_src(bad_url, None)


def test_widget_js_rejects_non_http_protocol(tmp_path):
    node = shutil.which("node")
    if node is None:
        pytest.skip("node not installed")
    mjs = tmp_path / "_widget.mjs"
    mjs.write_text(WIDGET_JS.read_text(encoding="utf-8"), encoding="utf-8")
    script = f"""
import widget from {mjs.as_uri()!r};
globalThis.window = {{ location: {{ href: "https://notebook.example/" }}, addEventListener() {{}}, removeEventListener() {{}} }};
globalThis.document = {{ createElement() {{ return {{ style: {{}} }}; }} }};
const badModel = {{ get: (k) => (k === "_src" ? "javascript:alert(1)" : ""), on() {{}}, off() {{}} }};
let threw = false;
try {{
  widget.render({{ model: badModel, el: {{ appendChild() {{}} }} }});
}} catch (e) {{
  threw = true;
}}
if (!threw) {{
  console.error("expected _widget.js to reject javascript: URL");
  process.exit(1);
}}
"""
    proc = subprocess.run([node, "--input-type=module", "-e", script], capture_output=True, text=True)
    assert proc.returncode == 0, proc.stderr
