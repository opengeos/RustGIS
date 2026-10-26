"""The RustGIS app in a Jupyter notebook: an anywidget around a :class:`~rustgis.Session`.

The widget shows the RustGIS web app (ribbon, panes, map) in the cell, kept in step with the
Python session through the project: what Python adds or styles appears in the app, and what is
done in the app (an edit, a new layer, a pan) is back in the session for Python to read.
"""

from __future__ import annotations

from typing import Any, Dict, Iterable, Optional, Union

import anywidget
import traitlets

from . import PathLike, Session
from ._embed import DEFAULT_URL, EMBED_LIMIT, WIDGET_JS, _app_src, _Embedded

__all__ = ["DEFAULT_URL", "EMBED_LIMIT", "Map"]


class Map(anywidget.AnyWidget, _Embedded):
    """The RustGIS app in a notebook cell, driven from Python.

    It has the methods of :class:`~rustgis.Session` (``run``, ``run_tool``, ``add_data``,
    ``m["Cities"]``...); what they change shows in the app.

    Args:
        session: The session to show; a new one when left out.
        sample: Open the World Atlas sample project.
        data: Files to add as layers.
        height: The widget's height, as CSS (``"800px"``).
        lang: The interface language (``"de"``, ``"ja"``, ...); the browser's when left out.
        url: The web app to show; ``RUSTGIS_WEB_URL`` or https://web.rustgis.app when left out.
        **kwargs: Passed on to ``anywidget.AnyWidget``.

    Example:
        >>> m = rustgis.Map(sample=True)
        >>> m["Countries"].style("graduated", field="pop_est")
        >>> m
    """

    _esm = WIDGET_JS
    _src = traitlets.Unicode("").tag(sync=True)
    _project = traitlets.Unicode("").tag(sync=True)
    height = traitlets.Unicode("800px").tag(sync=True)

    def __init__(
        self,
        session: Optional[Session] = None,
        *,
        sample: bool = False,
        data: Optional[Union[PathLike, Iterable[PathLike]]] = None,
        height: str = "800px",
        lang: Optional[str] = None,
        url: Optional[str] = None,
        **kwargs: Any,
    ) -> None:
        """Starts the widget on ``session`` or a new session."""
        super().__init__(height=height, _src=_app_src(url, lang), **kwargs)
        self._start(session, sample, data)
        self.on_msg(self._on_app)

    def _send_project(self, text: str) -> None:
        """Nothing to do: ``_project`` is synced to the page, which sends it on.

        Args:
            text: The project as JSON text.
        """

    def _send_run(self, message: Dict[str, Any]) -> None:
        """Sends a run message to the page.

        Args:
            message: The message.
        """
        self.send(message)

    def _on_app(self, _widget: Any, content: Any, _buffers: Any) -> None:
        """Takes the page's messages (see :meth:`_Embedded._from_app`).

        Args:
            _widget: This widget.
            content: The message.
            _buffers: Binary buffers (unused).
        """
        self._from_app(content)
