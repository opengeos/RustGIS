# User Guide

This guide walks through RustGIS by task. Each page covers one part of the app:

| Page | What it covers |
| --- | --- |
| [Data](data.md) | Vector and raster formats, adding data, raster display, project items |
| [Maps and Basemaps](maps.md) | Basemaps and web tile layers, projections, the 3D globe, view effects and navigation |
| [Cartography](cartography.md) | Symbology, labeling, layer effects and graphics |
| [Editing](editing.md) | Creating and modifying features, data design, the clipboard and pop-ups |
| [Analysis](analysis.md) | Selection and SQL queries, geoprocessing, the GeoLibre tools, guided workflows, imagery |
| [Layouts and Reports](layouts.md) | Page layouts, map series, reports, presentations and notebooks |
| [Sharing](sharing.md) | Exports, packages and open-format web maps, layers, styles and scenes |
| [Workspace](workspace.md) | Linked views, the Catalog view, animation, profiles, offline areas, customizing the ribbon |
| [AI Assistant](assistant.md) | A chat pane that answers questions and acts on the map through commands |

![RustGIS with the Map tab of the ribbon open over a world map in the Equal Earth projection, countries shaded by population](https://assets.rustgis.app/images/hero.webp)

## The window

- **Ribbon**: tabs of commands grouped by task. The **Project** button opens the backstage (new,
  open, save, About).
- **Quick Access Toolbar** (top left): New, Open, Save, Undo and Redo, plus any commands you add
  under **Help › Customize › Quick Access**.
- **Contents pane** (left): the map's layers in drawing order, with their legends. Tick a layer to
  show or hide it; right-click it to move it, zoom to it, rename or remove it and more.
  Selecting a layer opens its contextual tabs.
- **Map view**: one tab per map and layout. The buttons at the top right zoom in, zoom out and go
  to the full extent; the north arrow and scale bar sit in the corners; tile credits at the bottom
  right.
- **Panes** (right): Catalog, Geoprocessing, Symbology, History, Details, Editing, Workflows,
  Script, the AI Assistant and more. Open them from **View › Windows** or the commands that use
  them.
- **Status bar**: the scale (type one, or pick from the list), the coordinate system (click it to
  change the projection), the pointer's coordinates (click them to switch between decimal
  degrees and DMS), the selection count and the draw time.

**Command Search** (++ctrl+k++, or the box at the top right) finds any command or geoprocessing
tool by name and runs it.

To give the map more room, collapse the ribbon to its tab row with the chevron at its right
end, by double-clicking a tab, or with ++ctrl+f1++ (++cmd+f1++ on macOS, where F1 may need the Fn
key). Click a tab to peek at its commands while collapsed. When the window is narrower than the
ribbon, arrows at its ends scroll it.

## The ribbon

| Tab | Groups |
| --- | --- |
| **Map** | Clipboard, Navigate (Explore, Bookmarks, extents, Go To XY, Pop-ups), Layer (Basemap, Add Data), Selection, Inquiry (Measure, Locate, Infographics), Offline, Feature Cache, Labeling |
| **Insert** | Project (New Map, New Layout, Toolbox, Import, Add Folder, Templates, Task), Report, Presentation, Notebook, Layer Templates, Styles, Bookmarks, Graphics, Graphics and Text, Data |
| **Analysis** | Geoprocessing (History, Tools, ModelBuilder, Script, Environments, Ready To Use), AI (AI Assistant), Portal (Feature and Raster Analysis), Workflows, Raster (Raster Functions, Function Editor), the tool gallery, Selection |
| **View** | View (Globe, Projection, Global, Local), Effects, Scene, Windows, Display, Navigation, Link, Workspace, Thumbnail, Animation, Profile |
| **Edit** | Manage Edits (Edit, Save, Undo), Snapping, Features (Create, Modify, Delete), Tools, Elevation, Corrections, Selection, Fields |
| **Imagery** | Ortho Mapping, Reality Mapping, Alignment, Analysis, Image Classification, Share, Mensuration, Tools |
| **Share** | Output (Export Map, Print, Capture), Package, Share As, Layout, Save As |
| **Plugins** | The Plugin Manager, plus a tab for each [plugin](../plugins.md) that adds one |
| **Settings** | Language (interface language and worldview) |
| **Help** | Customize (ribbon tabs, Quick Access Toolbar), Performance (Diagnostic Monitor), Learn, Help (About, Command Search, Shortcuts) |

Contextual tabs appear while something is selected:

| Group | Tabs | Shown when |
| --- | --- | --- |
| **Feature Layer** | Feature Layer (visibility range, transparency, symbology, drawing effects, extrusion), Labeling (visibility range, placement, label class, text symbol), Data (definition query, table, selection, data design, archiving, relationships, export) | A vector layer is selected in Contents |
| **Raster Layer** | Raster Layer (visibility range, transparency, swipe, raster effects, rendering, enhancement), Data (information, export) | A raster layer is selected |
| **Layout** | Layout (page, map frames, map surrounds, text, export) | A layout tab is active |

## Projects

A project (`.rgisx`) holds maps, layouts, reports, presentations, notebooks, styles, tasks and
models. Vector data added from files is stored inside the project, so a project file is
self-contained; rasters are referenced by path on the desktop and stored inside the project in
the web app. **Project › New** starts a blank map or the World Atlas sample; **Save** and **Save
As** write the project. See [Data › Project items](data.md#project-items) for the other files
RustGIS reads and writes.

Every change is a command, and every command can be undone (++ctrl+z++) and redone
(++ctrl+y++).

## Keyboard shortcuts

| Keys | Action |
| --- | --- |
| ++ctrl+k++ | Command search |
| ++ctrl+n++ / ++ctrl+o++ / ++ctrl+s++ | New / Open / Save project |
| ++ctrl+z++ / ++ctrl+y++ | Undo / Redo |
| ++ctrl+t++ | Attribute table |
| ++ctrl+f1++ | Collapse or expand the ribbon |
| ++home++ | Full extent |
| ++esc++ | Back to Explore; cancel a sketch |
| ++delete++ | Delete selected features |
| ++ctrl+c++ / ++ctrl+x++ / ++ctrl+v++ | Copy / Cut / Paste features |

**Help › Shortcuts** lists them all in the app.
