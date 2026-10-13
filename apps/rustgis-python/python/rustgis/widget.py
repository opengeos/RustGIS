"""The RustGIS app in a Jupyter notebook: an anywidget around a :class:`~rustgis.Session`.

The widget shows the RustGIS web app (ribbon, panes, map) in the cell, kept in step with the
Python session through the project: what Python adds or styles appears in the app, and what is
done in the app (an edit, a new layer, a pan) is back in the session for Python to read.
"""

from __future__ import annotations

import base64
import os
import pathlib
from typing import Any, Dict, Iterable, List, Optional, Tuple, Union
from urllib.parse import quote

import anywidget
import traitlets

from . import Layer, PathLike, RustGISError, Session

#: The web app the widget shows; ``RUSTGIS_WEB_URL`` points it at another build (a local
#: ``trunk serve``, a self-hosted copy).
DEFAULT_URL = "https://web.rustgis.app/"

#: Files up to this size travel with the project, so the app in the browser shows them too.
EMBED_LIMIT = 100 << 20

# The extensions `layer.addDataBytes` reads from a file's contents.
_EMBEDDABLE = {
    ".tif", ".tiff", ".asc", ".geojson", ".json", ".csv", ".tsv", ".txt", ".zip", ".kml", ".kmz",
    ".gpx", ".parquet", ".geoparquet", ".fgb", ".gpkg", ".mbtiles", ".xlsx", ".xls", ".ods",
}


class Map(anywidget.AnyWidget):
    """The RustGIS app in a notebook cell, driven from Python.

    Args:
        session: The session to show; a new one when left out.
        sample: Open the World Atlas sample project.
        data: Files to add as layers.
        height: The widget's height, as CSS (``"600px"``).
        lang: The interface language (``"de"``, ``"ja"``, ...); the browser's when left out.
        url: The web app to show; ``RUSTGIS_WEB_URL`` or https://web.rustgis.app when left out.
        **kwargs: Passed on to ``anywidget.AnyWidget``.

    Example:
        >>> m = rustgis.Map(sample=True)
        >>> m.run("layer.symbology", layer="Countries", type="graduated", field="pop_est")
        >>> m
    """

    _esm = pathlib.Path(__file__).parent / "_widget.js"
    _src = traitlets.Unicode("").tag(sync=True)
    _project = traitlets.Unicode("").tag(sync=True)
    height = traitlets.Unicode("600px").tag(sync=True)

    def __init__(
        self,
        session: Optional[Session] = None,
        *,
        sample: bool = False,
        data: Optional[Union[PathLike, Iterable[PathLike]]] = None,
        height: str = "600px",
        lang: Optional[str] = None,
        url: Optional[str] = None,
        **kwargs: Any,
    ) -> None:
        """Starts the widget on ``session`` or a new session."""
        super().__init__(height=height, _src=_app_src(url, lang), **kwargs)
        self._session = session if session is not None else Session()
        self._held: List[Tuple[str, bool]] = []
        if sample:
            self._session.run("project.sample")
        if data is not None:
            for path in [data] if isinstance(data, (str, os.PathLike)) else data:
                command, args = _data_args(path)
                self._session.run(command, args)
        self._push()
        self.on_msg(self._on_app)

    @property
    def session(self) -> Session:
        """The session the widget shows."""
        return self._session

    # Commands ------------------------------------------------------------------------------

    def run(self, command: str, params: Optional[Dict[str, Any]] = None, /, **kwargs: Any) -> Any:
        """Runs a RustGIS command by id, then shows the result in the app.

        Args:
            command: The command id, e.g. ``"layer.symbology"``.
            params: The command's parameters, as a dict (positional only).
            **kwargs: Parameters as keyword arguments.

        Returns:
            The command's result, decoded from JSON.

        Raises:
            RustGISError: When the command fails.
        """
        try:
            return self._session.run(command, params, **kwargs)
        finally:
            self._push()

    def run_in_app(self, command: str, params: Optional[Dict[str, Any]] = None, /, **kwargs: Any) -> None:
        """Runs a command in the app shown, rather than in the session: the commands only the
        app has (``ui.*``: panes, the attribute table, the interface language).

        The command runs once the app has started; if it fails, the error is among
        :meth:`notices`.

        Args:
            command: The command id, e.g. ``"ui.language"``.
            params: The command's parameters, as a dict (positional only).
            **kwargs: Parameters as keyword arguments.
        """
        merged: Dict[str, Any] = dict(params or {})
        merged.update(kwargs)
        self.send({"type": "run", "command": command, "params": merged})

    def commands(self) -> List[Dict[str, Any]]:
        """Every command: its id, label, ribbon location and parameters."""
        return self._session.commands()

    def tools(self) -> Any:
        """The geoprocessing tools, with their parameters."""
        return self._session.tools()

    def notices(self) -> List[Tuple[str, bool]]:
        """What the session and the app have reported since the last call.

        Returns:
            ``(text, is_error)`` pairs, oldest first.
        """
        held, self._held = self._held, []
        return held + self._session.notices()

    def undo(self) -> Any:
        """Undoes the last change made from Python."""
        return self.run("edit.undo")

    def redo(self) -> Any:
        """Redoes the last undone change."""
        return self.run("edit.redo")

    # Projects ------------------------------------------------------------------------------

    def load_project(self, path: PathLike) -> Any:
        """Opens a project file (``.rgis``), replacing the current project.

        Args:
            path: The project file.
        """
        return self.run("project.open", path=os.fspath(path))

    def save(self, path: PathLike) -> Any:
        """Saves the project, with what was done in the app, to ``path`` (``.rgis``).

        Args:
            path: Where to save.
        """
        return self.run("project.saveAs", path=os.fspath(path))

    def info(self) -> Dict[str, Any]:
        """The project's summary: the active map, basemap, view and layers."""
        return self._session.info()

    # Layers --------------------------------------------------------------------------------

    def layers(self) -> List[Dict[str, Any]]:
        """The active map's layers, top first."""
        return self._session.layers()

    def layer(self, name: str) -> Layer:
        """Layer ``name`` of the active map.

        Args:
            name: The layer's name (or its id, as text).
        """
        return self._session.layer(name)

    def add_data(self, path: PathLike, name: Optional[str] = None, **params: Any) -> Any:
        """Adds a file as a layer.

        A file of up to 100 MB travels with the project, so the app shows it too; a larger one is
        read from its path, which only Python can see: it draws in :meth:`to_png` but not in the
        app.

        Args:
            path: The file (GeoJSON, CSV, Shapefile, GeoPackage, GeoParquet, GeoTIFF, ...).
            name: The layer's name (the file's name when left out).
            **params: More ``layer.addData`` parameters.
        """
        command, args = _data_args(path)
        if name is not None:
            params["name"] = name
        return self.run(command, **args, **params)

    def add_geojson(self, data: Union[Dict[str, Any], str, PathLike], name: str = "GeoJSON") -> Any:
        """Adds GeoJSON as a layer.

        Args:
            data: A GeoJSON dict, GeoJSON text, or the path of a GeoJSON file.
            name: The layer's name.
        """
        try:
            return self._session.add_geojson(data, name=name)
        finally:
            self._push()

    def add_geopandas(self, gdf: Any, name: str = "GeoDataFrame") -> Any:
        """Adds a ``geopandas.GeoDataFrame`` as a layer.

        Args:
            gdf: The GeoDataFrame.
            name: The layer's name.
        """
        try:
            return self._session.add_geopandas(gdf, name=name)
        finally:
            self._push()

    def add_tiles(self, url: str, name: Optional[str] = None, **params: Any) -> Any:
        """Adds a web tile layer: an XYZ template, a WMS, vector tiles or a MapLibre style.

        Args:
            url: The address, e.g. ``"https://tile.openstreetmap.org/{z}/{x}/{y}.png"``.
            name: The layer's name.
            **params: More ``map.addTiles`` parameters (``kind``, ``attribution``, ...).
        """
        if name is not None:
            params["name"] = name
        return self.run("map.addTiles", url=url, **params)

    # The view ------------------------------------------------------------------------------

    def set_center(self, lon: float, lat: float, scale: Optional[float] = None) -> Any:
        """Centres the map on a longitude and latitude.

        Args:
            lon: Longitude in degrees.
            lat: Latitude in degrees.
            scale: The map scale's denominator (``2_000_000`` for 1:2,000,000).
        """
        params: Dict[str, Any] = {"lon": lon, "lat": lat}
        if scale is not None:
            params["scale"] = scale
        return self.run("map.goToXY", params)

    def zoom_to(self, layer: str) -> Any:
        """Zooms the map to a layer's features.

        Args:
            layer: The layer's name.
        """
        return self.run("layer.zoomTo", layer=layer)

    def to_png(
        self,
        path: Optional[PathLike] = None,
        *,
        width: Optional[int] = None,
        height: Optional[int] = None,
        dpi: Optional[float] = None,
    ) -> bytes:
        """Renders the map as a PNG, as the session holds it.

        Args:
            path: Where to write the PNG; when left out only the bytes are returned.
            width: Width in pixels.
            height: Height in pixels.
            dpi: Resolution (96 when left out).

        Returns:
            The PNG bytes.
        """
        return self._session.to_png(path, width=width, height=height, dpi=dpi)

    # Keeping the app and the session in step -----------------------------------------------

    def _push(self) -> None:
        """Sends the session's project to the app, when it changed."""
        try:
            text = self._session._engine.project_json()
        except RustGISError as e:
            self._held.append((f"couldn't show the project in the app: {e}", True))
            return
        self._project = text

    def _on_app(self, _widget: Any, content: Any, _buffers: Any) -> None:
        """Takes the app's messages: its project after a change there, a command's failure.

        Args:
            _widget: This widget.
            content: The message.
            _buffers: Binary buffers (unused).
        """
        if not isinstance(content, dict):
            return
        kind = content.get("type")
        if kind == "project":
            text = content.get("text")
            if not isinstance(text, str) or text == self._project:
                return
            # Opening it reports "Opened ...": not news here, so it is dropped, and what was
            # waiting to be read is kept.
            self._held.extend(self._session.notices())
            try:
                self._session.run("project.openText", text=text)
            except RustGISError as e:
                self._held.append((f"the app's project couldn't be read: {e}", True))
                return
            finally:
                self._session.notices()
            # The app's own text, not the session's re-serialised copy: the app then sees its
            # project and doesn't open it again.
            self._project = text
        elif kind == "result" and content.get("ok") is False:
            self._held.append((str(content.get("error", "the command failed")), True))

    def __repr__(self) -> str:
        return f"Map(layers={[layer.get('name') for layer in self.layers()]!r})"


def _app_src(url: Optional[str], lang: Optional[str]) -> str:
    """The address of the app the widget frames: ``url`` with ``?embed`` (and ``lang``).

    Args:
        url: The web app's address, or None for ``RUSTGIS_WEB_URL`` or the default.
        lang: The interface language code, or None.
    """
    base = url or os.environ.get("RUSTGIS_WEB_URL") or DEFAULT_URL
    query = "embed" + (f"&lang={quote(lang)}" if lang else "")
    return base + ("&" if "?" in base else "?") + query


def _data_args(path: PathLike) -> Tuple[str, Dict[str, Any]]:
    """The command adding ``path``: its contents when they can travel with the project, else
    its path.

    Args:
        path: The file to add.

    Returns:
        The command id and its parameters.
    """
    p = pathlib.Path(os.fspath(path))
    try:
        small = p.is_file() and p.stat().st_size <= EMBED_LIMIT
    except OSError:
        small = False
    if small and p.suffix.lower() in _EMBEDDABLE:
        data = base64.b64encode(p.read_bytes()).decode("ascii")
        return "layer.addDataBytes", {"fileName": p.name, "data": data}
    return "layer.addData", {"path": os.fspath(path)}
