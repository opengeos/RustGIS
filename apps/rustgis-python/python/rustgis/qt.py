"""The RustGIS app in a Qt application: a widget around a :class:`~rustgis.Session`.

``MapWidget`` shows the RustGIS web app (ribbon, panes, map) in a Qt WebEngine view, kept in step
with the Python session through the project, as :class:`rustgis.Map` does in a notebook. It needs
PySide6 or PyQt6 with Qt WebEngine (``pip install "rustgis[qt]"``); import this module before the
``QApplication`` is created.

The view loads a small host page that runs the notebook widget's script (``_widget.js``) against a
stand-in for the anywidget model, which talks to Python over a ``QWebChannel``. The host page has
an address of its own (:data:`HOST_URL`) because the app answers only the origin that wrote to it
first.
"""

from __future__ import annotations

import json
from typing import Any, Callable, Dict, Iterable, List, Optional, Union

try:
    from PySide6.QtCore import QFile, QIODevice, QObject, QSize, QUrl
    from PySide6.QtCore import Slot as _slot
    from PySide6.QtWebChannel import QWebChannel
    from PySide6.QtWebEngineWidgets import QWebEngineView
    from PySide6.QtWidgets import QVBoxLayout, QWidget
except ImportError:
    try:
        from PyQt6.QtCore import QFile, QIODevice, QObject, QSize, QUrl
        from PyQt6.QtCore import pyqtSlot as _slot
        from PyQt6.QtWebChannel import QWebChannel
        from PyQt6.QtWebEngineWidgets import QWebEngineView
        from PyQt6.QtWidgets import QVBoxLayout, QWidget
    except ImportError as e:
        raise ImportError('rustgis.qt needs PySide6 or PyQt6 with Qt WebEngine: pip install "rustgis[qt]"') from e

from . import PathLike, Session
from ._embed import DEFAULT_URL, WIDGET_JS, _app_src, _Embedded

__all__ = ["DEFAULT_URL", "HOST_URL", "MapWidget"]

#: The host page's address. Nothing is fetched from it: it only gives the page an origin.
HOST_URL = "https://rustgis-qt.localhost/"

_HOST_PAGE = """<!doctype html>
<html>
<head>
<meta charset="utf-8">
<style>html, body, #app { margin: 0; height: 100%; overflow: hidden; }</style>
<script>__QWEBCHANNEL__</script>
</head>
<body>
<div id="app"></div>
<script>
// A stand-in for the anywidget model that _widget.js expects; Python is behind `host`.
const state = { _src: __SRC__, _project: "", height: "100%" };
const listeners = {};
const fire = (event, ...args) => (listeners[event] || []).slice().forEach((f) => f(...args));
new QWebChannel(qt.webChannelTransport, async (channel) => {
  const host = channel.objects.rustgis;
  const model = {
    get: (key) => state[key],
    on: (event, f) => (listeners[event] = listeners[event] || []).push(f),
    off: (event, f) => { listeners[event] = (listeners[event] || []).filter((g) => g !== f); },
    send: (content) => host.receive(JSON.stringify(content)),
  };
  window.rustgisHost = {
    project(text) { state._project = text; fire("change:_project"); },
    custom(msg) { fire("msg:custom", msg, []); },
  };
  const code = new Blob([__WIDGET__], { type: "text/javascript" });
  const widget = await import(URL.createObjectURL(code));
  widget.default.render({ model, el: document.getElementById("app") });
  host.receive(JSON.stringify({ type: "host" }));
});
</script>
</body>
</html>
"""


class _Bridge(QObject):
    """The object the host page calls: it hands each message to the widget."""

    def __init__(self, receive: Callable[[str], None]) -> None:
        """Keeps the function that takes the page's messages.

        Args:
            receive: Called with each message's JSON text.
        """
        super().__init__()
        self._receive = receive

    @_slot(str)
    def receive(self, text: str) -> None:
        """Takes a message from the host page.

        Args:
            text: The message, as JSON text.
        """
        self._receive(text)


class MapWidget(QWidget, _Embedded):
    """The RustGIS app in a Qt widget, driven from Python.

    It has the methods of :class:`~rustgis.Session` (``run``, ``run_tool``, ``add_data``,
    ``w["Cities"]``...); what they change shows in the app, and what is done in the app is back
    in the session.

    Args:
        session: The session to show; a new one when left out.
        sample: Open the World Atlas sample project.
        data: Files to add as layers.
        lang: The interface language (``"de"``, ``"ja"``, ...); the system's when left out.
        url: The web app to show; ``RUSTGIS_WEB_URL`` or https://web.rustgis.app when left out.
        parent: The parent widget.

    Example:
        >>> from rustgis.qt import MapWidget  # before the QApplication
        >>> app = QApplication([])
        >>> w = MapWidget(sample=True)
        >>> w["Countries"].style("graduated", field="pop_est")
        >>> w.show()
        >>> app.exec()
    """

    def __init__(
        self,
        session: Optional[Session] = None,
        *,
        sample: bool = False,
        data: Optional[Union[PathLike, Iterable[PathLike]]] = None,
        lang: Optional[str] = None,
        url: Optional[str] = None,
        parent: Optional[QWidget] = None,
    ) -> None:
        """Starts the widget on ``session`` or a new session."""
        QWidget.__init__(self, parent)
        # Messages for the page wait until it has started.
        self._ready = False
        self._waiting: List[Dict[str, Any]] = []
        self._src = _app_src(url, lang)
        self._view = QWebEngineView(self)
        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.addWidget(self._view)
        self._bridge = _Bridge(self._on_page)
        self._channel = QWebChannel(self._view.page())
        self._channel.registerObject("rustgis", self._bridge)
        self._view.page().setWebChannel(self._channel)
        # A reload starts a page without `rustgisHost`: messages wait for its "host" again.
        self._view.loadStarted.connect(self._unready)
        # The page first: if it can't be built, the session is left as it was. setHtml takes up
        # to 2 MB; the page is Qt's qwebchannel.js and _widget.js, a few dozen KB.
        page = _host_page(self._src)
        self._start(session, sample, data)
        self._view.setHtml(page, QUrl(HOST_URL))

    @property
    def view(self) -> QWebEngineView:
        """The web view the app runs in."""
        return self._view

    def sizeHint(self) -> QSize:  # noqa: N802 (Qt's name)
        """A size the app's ribbon and panes fit in."""
        return QSize(1280, 800)

    def _send_project(self, text: str) -> None:
        """Sends the project to the page; before it has started, it gets it when it does.

        Args:
            text: The project as JSON text.
        """
        if self._ready:
            self._js(f"window.rustgisHost.project({json.dumps(text)})")

    def _send_run(self, message: Dict[str, Any]) -> None:
        """Sends a run message to the page, or keeps it until the page has started.

        Args:
            message: The message.
        """
        if self._ready:
            self._js(f"window.rustgisHost.custom({json.dumps(message)})")
        else:
            self._waiting.append(message)

    def _unready(self) -> None:
        """Holds messages for the page until the one loading has started."""
        self._ready = False

    def _js(self, script: str) -> None:
        """Runs a script in the host page.

        Args:
            script: The JavaScript.
        """
        self._view.page().runJavaScript(script)

    def _on_page(self, text: str) -> None:
        """Takes a message from the host page: it has started, or one for the session.

        Args:
            text: The message, as JSON text.
        """
        try:
            content = json.loads(text)
        except ValueError:
            return
        if isinstance(content, dict) and content.get("type") == "host":
            # Started, or started again (reloaded): send it the project and what waited.
            self._ready = True
            if self._project:
                self._send_project(self._project)
            waiting, self._waiting = self._waiting, []
            for message in waiting:
                self._send_run(message)
            return
        self._from_app(content)

    # QWidget's own would come first.
    __repr__ = _Embedded.__repr__


def _host_page(src: str) -> str:
    """The host page for the app at ``src``.

    Args:
        src: The app's address, with ``?embed``.
    """

    def literal(value: str) -> str:
        # A JavaScript string inside a <script> element: "</" would end the element.
        return json.dumps(value).replace("</", "<\\/")

    return (
        _HOST_PAGE.replace("__QWEBCHANNEL__", _qwebchannel_js())
        .replace("__SRC__", literal(src))
        .replace("__WIDGET__", literal(WIDGET_JS.read_text(encoding="utf-8")))
    )


def _qwebchannel_js() -> str:
    """Qt's ``qwebchannel.js``, read from Qt's resources, so the page loads nothing for it."""
    f = QFile(":/qtwebchannel/qwebchannel.js")
    if not f.open(QIODevice.OpenModeFlag.ReadOnly):
        raise RuntimeError("Qt's qwebchannel.js wasn't found in its resources")
    try:
        return bytes(f.readAll().data()).decode("utf-8")
    finally:
        f.close()
