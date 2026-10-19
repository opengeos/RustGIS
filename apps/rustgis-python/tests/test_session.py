"""Tests for the rustgis Python package (run after `maturin develop`)."""

import json

import pytest

import rustgis


POINTS = {"type": "FeatureCollection", "features": [
    {"type": "Feature", "geometry": {"type": "Point", "coordinates": [114.17, 22.3]}, "properties": {"name": "Hong Kong"}},
    {"type": "Feature", "geometry": {"type": "Point", "coordinates": [113.26, 23.13]}, "properties": {"name": "Guangzhou"}},
]}


@pytest.fixture
def sample():
    """A session with the World Atlas sample open."""
    return rustgis.Session(sample=True)


def test_version_matches_the_engine():
    assert rustgis.__version__.count(".") >= 2


def test_sample_layers(sample):
    names = [layer["name"] for layer in sample.layers()]
    assert {"Cities", "Countries"} <= set(names)
    assert sample.layer("Cities").info["features"] > 200


def test_commands_and_tools_are_listed(sample):
    ids = {c["id"] for c in sample.commands()}
    assert {"layer.addData", "gp.run", "project.saveAs"} <= ids
    assert "buffer" in json.dumps(sample.tools())


def test_run_takes_keywords_including_a_params_keyword(sample):
    result = sample.run("gp.run", tool="buffer", params={"input": "Cities", "distance": 100})
    assert "layer" in result
    assert any("Buffer" in text for text, _ in sample.notices())
    assert sample.notices() == []
    # A dict works too, and keywords win over it.
    sample.run("layer.transparency", {"layer": "Countries", "value": 10}, value=40)
    assert any(l["name"] == "Countries" for l in sample.layers())


def test_errors_raise_rustgis_error(sample):
    with pytest.raises(rustgis.RustGISError, match="unknown command"):
        sample.run("no.such.command")
    with pytest.raises(rustgis.RustGISError):
        sample.layer("No such layer").to_geojson()


def test_parameters_must_be_json(sample):
    with pytest.raises(ValueError, match="finite"):
        sample.run("map.setScale", scale=float("nan"))


def test_geojson_round_trip(sample):
    collection = sample.layer("Cities").to_geojson()
    assert collection["type"] == "FeatureCollection" and len(collection["features"]) > 200
    big = {"type": "FeatureCollection", "features": [f for f in collection["features"] if f["properties"]["pop_max"] > 10_000_000]}
    sample.add_geojson(big, name="Megacities")
    assert sample.layer("Megacities").info["features"] == len(big["features"])
    sample.undo()
    assert "Megacities" not in [l["name"] for l in sample.layers()]


def test_png(sample, tmp_path):
    out = tmp_path / "map.png"
    png = sample.to_png(out, width=400, height=200)
    assert png[:4] == b"\x89PNG" and out.read_bytes() == png
    assert sample._repr_png_()[:4] == b"\x89PNG"


def test_projects_save_and_open(sample, tmp_path):
    path = tmp_path / "atlas.rgis"
    sample.run("gp.run", tool="buffer", params={"input": "Cities", "distance": 50})
    sample.save(path)
    reopened = rustgis.Session(path)
    assert [l["name"] for l in reopened.layers()] == [l["name"] for l in sample.layers()]


def test_data_files(tmp_path):
    path = tmp_path / "points.geojson"
    path.write_text(json.dumps({"type": "FeatureCollection", "features": [
        {"type": "Feature", "geometry": {"type": "Point", "coordinates": [114.17, 22.3]}, "properties": {"name": "Hong Kong"}},
        {"type": "Feature", "geometry": {"type": "Point", "coordinates": [113.26, 23.13]}, "properties": {"name": "Guangzhou"}},
    ]}))
    s = rustgis.Session(data=path)
    assert s.layers()[0]["features"] == 2


def test_geopandas_round_trip(sample):
    gpd = pytest.importorskip("geopandas")
    gdf = sample.layer("Cities").to_geopandas()
    assert isinstance(gdf, gpd.GeoDataFrame) and gdf.crs.to_epsg() == 4326
    sample.add_geopandas(gdf.head(5), name="Five")
    assert sample.layer("Five").info["features"] == 5


def test_a_session_is_usable_from_another_thread():
    """Jupyter delivers widget messages on a thread of its own."""
    import threading

    s = rustgis.Session(sample=True)
    seen = []
    t = threading.Thread(target=lambda: seen.append(len(s.layers())))
    t.start()
    t.join()
    assert seen == [len(s.layers())]


def test_project_json_reopens_as_the_same_project():
    s = rustgis.Session(sample=True)
    other = rustgis.Session()
    other.run("project.openText", text=s.project_json())
    assert [layer["name"] for layer in other.layers()] == [layer["name"] for layer in s.layers()]


def test_layers_by_name_and_id(sample):
    cities = sample["Cities"]
    assert cities == sample.layer(cities.id) == sample["cities"]
    assert cities.type == "Point" and cities.count > 200 and "pop_max" in cities.fields
    west, south, east, north = cities.bounds
    assert west < east and south < north
    assert "Cities" in sample and cities in sample and "No such layer" not in sample
    assert [layer.name for layer in sample] == sample.layer_names()
    with pytest.raises(KeyError):
        sample["No such layer"]


def test_a_layer_is_the_same_after_a_rename(sample):
    cities = sample["Cities"]
    cities.rename("Towns")
    assert cities.name == "Towns" and "Cities" not in sample
    cities.visible = False
    assert not cities.visible and not sample["Towns"].info["visible"]
    cities.remove()
    assert "Towns" not in sample
    with pytest.raises(rustgis.RustGISError):
        cities.name
    assert "removed" in repr(cities)


def test_styling_filtering_and_selection(sample):
    countries = sample["Countries"].style("graduated", field="pop_est", classes=5)
    assert "Graduated" in countries.info["renderer"]
    cities = sample["Cities"]
    n = cities.select("pop_max > 10000000")
    assert n == cities.selected > 0
    selected = cities.to_geojson(selected=True)["features"]
    assert len(selected) == n and all(f["properties"]["pop_max"] > 10_000_000 for f in selected)
    cities.clear_selection()
    assert cities.selected == 0 and cities.to_geojson(selected=True)["features"] == []
    cities.filter("pop_max > 10000000")
    assert cities.info["definitionQuery"]
    cities.filter(None)
    assert not cities.info["definitionQuery"]


def test_run_tool_returns_its_layer(sample):
    buffers = sample.run_tool("buffer", input=sample["Cities"], distance=100)
    assert isinstance(buffers, rustgis.Layer) and buffers.type == "Polygon"
    assert buffers.count == sample["Cities"].count
    assert sample.describe("buffer")["name"] == "Buffer"
    assert "params" in sample.describe("layer.symbology")
    with pytest.raises(KeyError):
        sample.describe("no.such.thing")


def test_adding_data_returns_the_layer(sample, tmp_path):
    sites = sample.add_geojson(POINTS, name="Sites")
    assert isinstance(sites, rustgis.Layer) and sites.name == "Sites" and sites.count == 2
    out = tmp_path / "sites.gpkg"
    sites.to_file(out)
    again = sample.add_data(out, name="Again")
    assert again.count == 2 and again.name == "Again"
    sample.remove_layer("Again")
    assert "Again" not in sample


def test_the_view(sample):
    sample.zoom_to(sample["Cities"])
    sample.zoom_to([sample["Cities"], sample["Countries"]])
    with pytest.raises(TypeError):
        sample.zoom_to(True)
    with pytest.raises(TypeError):
        sample.zoom_to((0, 0))
    sample.zoom_to((0, 0, 10, 10))
    west, south, east, north = sample.bounds
    assert west <= 0 and east >= 10 and south <= 0 and north >= 10
    sample.set_center(114.17, 22.3, scale=1_000_000)
    assert abs(sample.info()["center"]["lon"] - 114.17) < 0.01
    sample.set_basemap("paper")
    assert sample.info()["basemap"] == "paper"



def test_a_layer_named_like_another_layers_id_reads_its_own_features(sample):
    cities = sample["Cities"]
    decoy = sample.add_geojson(POINTS, name=str(cities.id))
    assert len(cities.to_geojson()["features"]) == cities.count != decoy.count
    assert decoy.to_geojson()["features"][0]["properties"]["name"] == "Hong Kong"
    assert True not in sample


def test_a_layer_of_another_session_is_refused(sample):
    other = rustgis.Session(sample=True)
    stranger = other["Cities"]
    assert stranger not in sample
    with pytest.raises(rustgis.RustGISError, match="another session"):
        sample.remove_layer(stranger)
    assert "Cities" in sample
