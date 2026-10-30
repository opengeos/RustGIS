# Changelog

## Unreleased

### Added

- Logins for web-service connections (WMS, WMTS, WFS, WCS, OGC API, ArcGIS REST, CSW, STAC):
  basic authentication or an API key in a header (#757).
  The connection keeps the method, never the password or key, which lasts for the session or is
  kept in the computer's credential store with **Remember on this computer**. A login is sent only
  to its connection's origin (scheme, host and port), never after a redirect to another origin.
  Sessions and connections have separate authenticated requests and caches; logout stays ended
  until an explicit login. A plain `http://` address needs confirmation (`allowInsecure`). New commands
  `connection.logout` and `connection.forgetLogin`; `connection.login`, `connection.new`,
  `connection.edit` and `connection.test` take the new fields. Layers added from a connection
  record its id in the project (`"connection"`, additive), and a project opened without the
  connection or its login asks for one.
