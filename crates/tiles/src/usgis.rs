//! The public GIS portals of US federal agencies, states and local governments, as catalogs to
//! search: an ArcGIS organisation (its public items) or a Socrata site. A snapshot of the
//! portals checked live in September 2026 (from GeoLibre's US GIS catalogs).

/// A portal: its name, its home page, and how it is searched.
pub struct Catalog {
    pub name: &'static str,
    pub url: &'static str,
    pub search: Search,
}

/// How a portal is searched.
pub enum Search {
    /// An ArcGIS organisation's public items, by its id.
    ArcGis(&'static str),
    /// A Socrata site, by its domain.
    Socrata(&'static str),
}

/// A department, state or other group of portals.
pub struct Set {
    pub name: &'static str,
    pub catalogs: &'static [Catalog],
}

/// US federal agencies' portals, by department.
pub const FEDERAL: &[Set] = &[
    Set {
        name: "Department of Agriculture",
        catalogs: &[
            Catalog {
                name: "Aerial Photography Field Office (NAIP)",
                url: "https://apfo-usdaonline.opendata.arcgis.com",
                search: Search::ArcGis("LLVEmB8Lsae3Um4s"),
            },
            Catalog { name: "Forest Service", url: "https://data-usfs.hub.arcgis.com", search: Search::ArcGis("gGHDlz6USftL5Pau") },
            Catalog {
                name: "Natural Resources Conservation Service",
                url: "https://nrcs.maps.arcgis.com",
                search: Search::ArcGis("SXbDpmb7xQkk44JV"),
            },
            Catalog {
                name: "USDA Geospatial Hub",
                url: "https://gisforagriculture-usdaocio.hub.arcgis.com",
                search: Search::ArcGis("5vMtpwj1mnc06Rmi"),
            },
        ],
    },
    Set {
        name: "Department of Commerce",
        catalogs: &[
            Catalog { name: "Census Bureau", url: "https://uscensus.maps.arcgis.com", search: Search::ArcGis("DlJzJLOZpPXmMpWi") },
            Catalog { name: "NOAA GeoPlatform", url: "https://noaa.maps.arcgis.com", search: Search::ArcGis("C8EMgrsFcRFL6LrL") },
            Catalog { name: "NOAA Marine Cadastre", url: "https://hub.marinecadastre.gov", search: Search::ArcGis("C8EMgrsFcRFL6LrL") },
        ],
    },
    Set {
        name: "Department of Defense",
        catalogs: &[
            Catalog {
                name: "Army Corps of Engineers",
                url: "https://geospatial-usace.opendata.arcgis.com",
                search: Search::ArcGis("n1YM8pTrFmm7L4hs"),
            },
            Catalog {
                name: "National Geospatial-Intelligence Agency",
                url: "https://nga.maps.arcgis.com",
                search: Search::ArcGis("cc7nIINtrZ67dyVJ"),
            },
        ],
    },
    Set {
        name: "Department of Energy",
        catalogs: &[Catalog { name: "EIA U.S. Energy Atlas", url: "https://atlas.eia.gov", search: Search::ArcGis("FGr1D95XCGALKXqM") }],
    },
    Set {
        name: "Department of Homeland Security",
        catalogs: &[Catalog { name: "FEMA", url: "https://gis-fema.hub.arcgis.com", search: Search::ArcGis("XG15cJAlne2vxtgt") }],
    },
    Set {
        name: "Department of Housing and Urban Development",
        catalogs: &[Catalog {
            name: "HUD Open Data",
            url: "https://hudgis-hud.opendata.arcgis.com",
            search: Search::ArcGis("VTyQ9soqVukalItT"),
        }],
    },
    Set {
        name: "Department of the Interior",
        catalogs: &[
            Catalog {
                name: "Bureau of Land Management",
                url: "https://gbp-blm-egis.hub.arcgis.com",
                search: Search::ArcGis("KbxwQRRfWyEYLgp4"),
            },
            Catalog {
                name: "Bureau of Ocean Energy Management",
                url: "https://boem-metaport-boem.hub.arcgis.com",
                search: Search::ArcGis("G5Ma95RzqJRPKsWL"),
            },
            Catalog {
                name: "Fish and Wildlife Service",
                url: "https://gis-fws.opendata.arcgis.com",
                search: Search::ArcGis("QVENGdaPbd4LUkLV"),
            },
            Catalog {
                name: "National Interagency Fire Center",
                url: "https://data-nifc.opendata.arcgis.com",
                search: Search::ArcGis("T4QMspbfLg3qTGWY"),
            },
            Catalog {
                name: "National Park Service",
                url: "https://public-nps.opendata.arcgis.com",
                search: Search::ArcGis("fBc8EJBxQRMcHlei"),
            },
            Catalog { name: "U.S. Geological Survey", url: "https://usgs.maps.arcgis.com", search: Search::ArcGis("v01gqwM5QqNysAAi") },
        ],
    },
    Set {
        name: "Department of Transportation",
        catalogs: &[
            Catalog { name: "BTS Geospatial (NTAD)", url: "https://geodata.bts.gov", search: Search::ArcGis("xOi1kZaI0eWDREZv") },
            Catalog {
                name: "FAA Aeronautical Data",
                url: "https://adds-faa.opendata.arcgis.com",
                search: Search::ArcGis("ssFJjBXIUyZDrSYZ"),
            },
        ],
    },
    Set {
        name: "Environmental Protection Agency",
        catalogs: &[Catalog { name: "U.S. EPA", url: "https://epa.maps.arcgis.com", search: Search::ArcGis("cJ9YHowT8TU7DUyn") }],
    },
    Set {
        name: "Multi-agency",
        catalogs: &[
            Catalog {
                name: "Esri U.S. Federal Datasets",
                url: "https://fedmaps.maps.arcgis.com",
                search: Search::ArcGis("FiaPA4ga0iQKduv3"),
            },
            Catalog { name: "GeoPlatform (NGDA)", url: "https://geoplatform.maps.arcgis.com", search: Search::ArcGis("Hp6G80Pky0om7QvQ") },
        ],
    },
    Set {
        name: "NASA",
        catalogs: &[Catalog {
            name: "NASA ArcGIS Online",
            url: "https://nasa.maps.arcgis.com",
            search: Search::ArcGis("WSiUmUhlFx4CtMBB"),
        }],
    },
];

/// US states' portals (and the District of Columbia).
pub const STATE: &[Set] = &[
    Set {
        name: "Alabama",
        catalogs: &[Catalog {
            name: "Alabama GeoHub",
            url: "https://data-algeohub.opendata.arcgis.com",
            search: Search::ArcGis("jF2q3LPxL7PETdYk"),
        }],
    },
    Set {
        name: "Alaska",
        catalogs: &[
            Catalog { name: "Alaska Geoportal", url: "https://akgeoportal.alaska.gov", search: Search::ArcGis("7HDiw78fcUiM2BWn") },
            Catalog {
                name: "Alaska DOT&PF Open Data",
                url: "https://data-soa-akdot.opendata.arcgis.com",
                search: Search::ArcGis("r4A0V7UzH9fcLVvv"),
            },
        ],
    },
    Set {
        name: "Arizona",
        catalogs: &[Catalog {
            name: "AZGeo Data Hub",
            url: "https://azgeo-data-hub-agic.hub.arcgis.com",
            search: Search::ArcGis("MrgGZS7oC7drh1E8"),
        }],
    },
    Set {
        name: "Arkansas",
        catalogs: &[Catalog {
            name: "Arkansas GIS Hub",
            url: "https://geodata.gis.arkansas.gov",
            search: Search::ArcGis("PwY9ZuZRDiI5nXUB"),
        }],
    },
    Set {
        name: "California",
        catalogs: &[Catalog {
            name: "California State Geoportal",
            url: "https://gis.data.ca.gov",
            search: Search::ArcGis("uknczv4rpevve42E"),
        }],
    },
    Set {
        name: "Colorado",
        catalogs: &[
            Catalog { name: "Colorado Geospatial Portal", url: "https://geodata.colorado.gov", search: Search::ArcGis("DgjqnJA1rgO92Soi") },
            Catalog { name: "CDOT Open Data", url: "https://data-cdot.opendata.arcgis.com", search: Search::ArcGis("yzB9WM8W0BO3Ql7d") },
        ],
    },
    Set {
        name: "Connecticut",
        catalogs: &[
            Catalog { name: "CT Geospatial Data Portal", url: "https://geodata.ct.gov", search: Search::ArcGis("3FL1kr7L4LvwA2Kb") },
            Catalog {
                name: "CT DEEP GIS Open Data",
                url: "https://ct-deep-gis-open-data-website-ctdeep.hub.arcgis.com",
                search: Search::ArcGis("FjPcSmEFuDYlIdKC"),
            },
        ],
    },
    Set {
        name: "Delaware",
        catalogs: &[Catalog {
            name: "Delaware FirstMap",
            url: "https://de-firstmap-delaware.hub.arcgis.com",
            search: Search::ArcGis("bFDgLqS5IyUBQLAd"),
        }],
    },
    Set {
        name: "District of Columbia",
        catalogs: &[Catalog { name: "Open Data DC", url: "https://opendata.dc.gov", search: Search::ArcGis("neT9SoYxizqTHZPH") }],
    },
    Set {
        name: "Florida",
        catalogs: &[
            Catalog {
                name: "Florida Geospatial Open Data Portal",
                url: "https://geodata.floridagio.gov",
                search: Search::ArcGis("Gh9awoU677aKree0"),
            },
            Catalog {
                name: "Florida DEP Geospatial Open Data",
                url: "https://geodata.dep.state.fl.us",
                search: Search::ArcGis("nRHtyn3uE1kyzoYc"),
            },
            Catalog { name: "FDOT Open Data", url: "https://gis-fdot.opendata.arcgis.com", search: Search::ArcGis("O1JpcwDW8sjYuddV") },
        ],
    },
    Set {
        name: "Georgia",
        catalogs: &[
            Catalog { name: "Georgia GIO", url: "https://data-hub.gio.georgia.gov", search: Search::ArcGis("Za9Nk6CPIPbvR1t7") },
            Catalog {
                name: "Georgia DCA",
                url: "https://data-georgia-dca.opendata.arcgis.com",
                search: Search::ArcGis("Gqyymy5JISeLzyNM"),
            },
        ],
    },
    Set {
        name: "Hawaii",
        catalogs: &[Catalog {
            name: "Hawaii Statewide GIS Program",
            url: "https://geoportal.hawaii.gov",
            search: Search::ArcGis("HQ0xoN0EzDPBOEci"),
        }],
    },
    Set {
        name: "Idaho",
        catalogs: &[Catalog {
            name: "State of Idaho GIS",
            url: "https://gis-idaho.hub.arcgis.com",
            search: Search::ArcGis("CNPdEkvnGl65jCX8"),
        }],
    },
    Set {
        name: "Illinois",
        catalogs: &[Catalog {
            name: "IDOT Open Data",
            url: "https://gis-idot.opendata.arcgis.com",
            search: Search::ArcGis("aIrBD8yn1TDTEXoz"),
        }],
    },
    Set {
        name: "Indiana",
        catalogs: &[Catalog { name: "IndianaMap", url: "https://www.indianamap.org", search: Search::ArcGis("fAJUAyCk5qmGxuxo") }],
    },
    Set {
        name: "Iowa",
        catalogs: &[
            Catalog { name: "Iowa Geospatial Data", url: "https://geodata.iowa.gov", search: Search::ArcGis("vPD5PVLI6sfkZ5E4") },
            Catalog { name: "Iowa DOT Open Data", url: "https://data.iowadot.gov", search: Search::ArcGis("8lRhdTsQyJpO52F1") },
        ],
    },
    Set {
        name: "Kansas",
        catalogs: &[Catalog { name: "Kansas Geoportal", url: "https://hub.kansasgis.org", search: Search::ArcGis("q2CglofYX6ACNEeu") }],
    },
    Set {
        name: "Kentucky",
        catalogs: &[
            Catalog { name: "KyGovMaps Open Data", url: "https://opengisdata.ky.gov", search: Search::ArcGis("ghsX9CKghMvyYjBU") },
            Catalog { name: "KyFromAbove", url: "https://kyfromabove.ky.gov", search: Search::ArcGis("ghsX9CKghMvyYjBU") },
        ],
    },
    Set {
        name: "Louisiana",
        catalogs: &[
            Catalog { name: "Louisiana DOTD", url: "https://release-ladotd.hub.arcgis.com", search: Search::ArcGis("PLiuXYMBpMK5h36e") },
            Catalog {
                name: "Atlas: The Louisiana Statewide GIS",
                url: "https://atlas-lsuga.opendata.arcgis.com",
                search: Search::ArcGis("SDQDNhpG8jikA0D1"),
            },
        ],
    },
    Set {
        name: "Maine",
        catalogs: &[Catalog {
            name: "Maine GeoLibrary",
            url: "https://mainegeolibrary-maine.hub.arcgis.com",
            search: Search::ArcGis("RbMX0mRVOFNTdLzd"),
        }],
    },
    Set {
        name: "Maryland",
        catalogs: &[Catalog {
            name: "Maryland GIS Data Catalog",
            url: "https://data.imap.maryland.gov",
            search: Search::ArcGis("njFNhDsUCentVYJW"),
        }],
    },
    Set {
        name: "Massachusetts",
        catalogs: &[
            Catalog { name: "MassGIS Data Hub", url: "https://gis.data.mass.gov", search: Search::ArcGis("hGdibHYSPO59RG1h") },
            Catalog {
                name: "MassDOT Open Data",
                url: "https://geo-massdot.opendata.arcgis.com",
                search: Search::ArcGis("ceiitspzDAHrdGO1"),
            },
        ],
    },
    Set {
        name: "Michigan",
        catalogs: &[Catalog {
            name: "State of Michigan Open Data",
            url: "https://gis-michigan.opendata.arcgis.com",
            search: Search::ArcGis("dxRQUfTDNtfqZ301"),
        }],
    },
    Set {
        name: "Minnesota",
        catalogs: &[Catalog {
            name: "Minnesota Geospatial Commons",
            url: "https://gis.data.mn.gov",
            search: Search::ArcGis("9OIuDHbyhmH91RfZ"),
        }],
    },
    Set {
        name: "Mississippi",
        catalogs: &[Catalog {
            name: "Mississippi Geospatial Data Catalog",
            url: "https://opendata.gis.ms.gov",
            search: Search::ArcGis("XSDoE9o9b2LpxKKd"),
        }],
    },
    Set {
        name: "Missouri",
        catalogs: &[Catalog { name: "MSDIS", url: "https://data-msdis.opendata.arcgis.com", search: Search::ArcGis("kNS2ppBA4rwAQQZy") }],
    },
    Set {
        name: "Montana",
        catalogs: &[
            Catalog {
                name: "Montana Geographic Information",
                url: "https://montana-state-library-2022-floods-gis-data-hub-montana.hub.arcgis.com",
                search: Search::ArcGis("qnjIrwR8z5Izc0ij"),
            },
            Catalog { name: "Montana FWP", url: "https://gis-mtfwp.hub.arcgis.com", search: Search::ArcGis("Cdxz8r11hT0MGzg1") },
        ],
    },
    Set {
        name: "Nebraska",
        catalogs: &[Catalog { name: "NebraskaMAP", url: "https://www.nebraskamap.gov", search: Search::ArcGis("Sj9eBhzWwOMzQCfI") }],
    },
    Set {
        name: "Nevada",
        catalogs: &[Catalog {
            name: "Nevada Department of Wildlife",
            url: "https://nevada-department-of-wildlife-data-hub-ndow.hub.arcgis.com",
            search: Search::ArcGis("RyxlXSfFi87rAosq"),
        }],
    },
    Set {
        name: "New Hampshire",
        catalogs: &[
            Catalog { name: "NH GRANIT", url: "https://www.nhgeodata.unh.edu", search: Search::ArcGis("wnvDDrXX8EouLkZP") },
            Catalog {
                name: "NHDES Open Data",
                url: "https://nh-department-of-environmental-services-open-data-nhdes.hub.arcgis.com",
                search: Search::ArcGis("MAcUimSes4gPY4sM"),
            },
        ],
    },
    Set {
        name: "New Jersey",
        catalogs: &[
            Catalog {
                name: "NJ Office of GIS",
                url: "https://njogis-newjersey.opendata.arcgis.com",
                search: Search::ArcGis("XVOqAjTOJ5P6ngMu"),
            },
            Catalog {
                name: "NJDEP Open Data",
                url: "https://gisdata-njdep.opendata.arcgis.com",
                search: Search::ArcGis("QWdNfRs7lkPq4g4Q"),
            },
        ],
    },
    Set {
        name: "New Mexico",
        catalogs: &[
            Catalog {
                name: "New Mexico Environment Department",
                url: "https://data-nmenv.opendata.arcgis.com",
                search: Search::ArcGis("sqciGhV7WvGQn4ky"),
            },
            Catalog {
                name: "NM Office of the State Engineer",
                url: "https://geospatialdata-ose.opendata.arcgis.com",
                search: Search::ArcGis("qXZbWTdPDbTjl7Dy"),
            },
        ],
    },
    Set {
        name: "New York",
        catalogs: &[Catalog { name: "NYS GIS Clearinghouse", url: "https://data.gis.ny.gov", search: Search::ArcGis("EbVsqZ18sv1kVJ3k") }],
    },
    Set {
        name: "North Carolina",
        catalogs: &[Catalog { name: "NC OneMap", url: "https://www.nconemap.gov", search: Search::ArcGis("mSDBiLWaIfH92NqI") }],
    },
    Set {
        name: "North Dakota",
        catalogs: &[Catalog {
            name: "North Dakota GIS Hub",
            url: "https://gishubdata-ndgov.hub.arcgis.com",
            search: Search::ArcGis("GOcSXpzwBHyk2nog"),
        }],
    },
    Set {
        name: "Ohio",
        catalogs: &[
            Catalog { name: "Ohio OGRIP", url: "https://ogriphome-geohio.hub.arcgis.com", search: Search::ArcGis("MlJ0G8iWUyC7jAmu") },
            Catalog { name: "Ohio DNR", url: "https://gis-odnr.opendata.arcgis.com", search: Search::ArcGis("ajRlmtxbNBjZggOT") },
        ],
    },
    Set {
        name: "Oklahoma",
        catalogs: &[Catalog {
            name: "Oklahoma Open Data",
            url: "https://oklahoma-open-data-oklahoma.hub.arcgis.com",
            search: Search::ArcGis("AFMO17kGL5gMkuoK"),
        }],
    },
    Set {
        name: "Oregon",
        catalogs: &[Catalog { name: "Oregon GEOHub", url: "https://geohub.oregon.gov", search: Search::ArcGis("8PAo5HGmvRMlF2eU") }],
    },
    Set {
        name: "Pennsylvania",
        catalogs: &[
            Catalog { name: "PennShare", url: "https://data-pennshare.opendata.arcgis.com", search: Search::ArcGis("jOy9iZUXBy03ojXb") },
            Catalog { name: "PA DCNR", url: "https://newdata-dcnr.opendata.arcgis.com", search: Search::ArcGis("CPq7UDkBXVRqtgLR") },
        ],
    },
    Set {
        name: "Rhode Island",
        catalogs: &[Catalog { name: "RIGIS", url: "https://www.rigis.org", search: Search::ArcGis("S8zZg9pg23JUEexQ") }],
    },
    Set {
        name: "South Carolina",
        catalogs: &[Catalog {
            name: "SC Department of Natural Resources",
            url: "https://data-scdnr.opendata.arcgis.com",
            search: Search::ArcGis("acgZYxoN5Oj8pDLa"),
        }],
    },
    Set {
        name: "South Dakota",
        catalogs: &[Catalog {
            name: "State of South Dakota",
            url: "https://gis-south-dakota-open-data-hub-sdbit.hub.arcgis.com",
            search: Search::ArcGis("PwrabBhZHUggYYSp"),
        }],
    },
    Set {
        name: "Tennessee",
        catalogs: &[Catalog {
            name: "Tennessee Downloadable GIS Data",
            url: "https://geodata.tn.gov",
            search: Search::ArcGis("YuVBSS7Y1of2Qud1"),
        }],
    },
    Set {
        name: "Texas",
        catalogs: &[Catalog {
            name: "TxDOT Open Data",
            url: "https://gis-txdot.opendata.arcgis.com",
            search: Search::ArcGis("KTcxiTD9dsQw4r7Z"),
        }],
    },
    Set {
        name: "Utah",
        catalogs: &[Catalog {
            name: "Utah SGID Open Data",
            url: "https://opendata.gis.utah.gov",
            search: Search::ArcGis("99lidPhWCzftIe9K"),
        }],
    },
    Set {
        name: "Vermont",
        catalogs: &[Catalog {
            name: "Vermont Open Geodata Portal",
            url: "https://geodata.vermont.gov",
            search: Search::ArcGis("BkFxaEFNwHqX3tAw"),
        }],
    },
    Set {
        name: "Virginia",
        catalogs: &[Catalog {
            name: "VGIN Clearinghouse",
            url: "https://vgin.vdem.virginia.gov",
            search: Search::ArcGis("WNJF1HhUVAm3jccO"),
        }],
    },
    Set {
        name: "Washington",
        catalogs: &[
            Catalog { name: "Washington Geospatial Open Data", url: "https://geo.wa.gov", search: Search::ArcGis("jsIt88o09Q0r1j8h") },
            Catalog {
                name: "WSDOT Open Data",
                url: "https://gisdata-wsdot.opendata.arcgis.com",
                search: Search::ArcGis("IYrj3otxNjPsrTRD"),
            },
        ],
    },
    Set {
        name: "West Virginia",
        catalogs: &[Catalog {
            name: "WV GTI Map & Data Portal",
            url: "https://data-wvdot.opendata.arcgis.com",
            search: Search::ArcGis("xLpB90lOmCXYDAWo"),
        }],
    },
    Set {
        name: "Wisconsin",
        catalogs: &[Catalog {
            name: "Wisconsin DNR Open Data",
            url: "https://data-wi-dnr.opendata.arcgis.com",
            search: Search::ArcGis("Ul9AyFFeFTjf08DW"),
        }],
    },
    Set {
        name: "Wyoming",
        catalogs: &[Catalog { name: "Wyoming GeoHub", url: "https://data.geospatialhub.org", search: Search::ArcGis("HVjI8GKrRtjcQ4Ry") }],
    },
];

/// US counties' and cities' portals, by state.
pub const LOCAL: &[Set] = &[
    Set {
        name: "Alabama",
        catalogs: &[Catalog {
            name: "Jefferson County (Birmingham)",
            url: "https://data-jeffco-al.opendata.arcgis.com",
            search: Search::ArcGis("2FZPLYt8NeStZfN0"),
        }],
    },
    Set {
        name: "Arizona",
        catalogs: &[
            Catalog { name: "Chandler", url: "https://share-open-data-changis.hub.arcgis.com", search: Search::ArcGis("HIBNcuytta1apnkB") },
            Catalog {
                name: "Maricopa County",
                url: "https://data-maricopa.opendata.arcgis.com",
                search: Search::ArcGis("ykpntM6e3tHvzKRJ"),
            },
            Catalog { name: "Mesa", url: "https://data.mesaaz.gov", search: Search::Socrata("data.mesaaz.gov") },
            Catalog { name: "Phoenix", url: "https://mapping-phoenix.opendata.arcgis.com", search: Search::ArcGis("cfKakmeHE95cgeEK") },
            Catalog { name: "Pima County", url: "https://gisopendata.pima.gov", search: Search::ArcGis("UTBp78iglGpbqp1B") },
            Catalog { name: "Scottsdale", url: "https://data.scottsdaleaz.gov", search: Search::ArcGis("hvDuIN6e7bxkHpdO") },
            Catalog { name: "Tempe", url: "https://data.tempe.gov", search: Search::ArcGis("lQySeXwbBg53XWDi") },
            Catalog { name: "Tucson", url: "https://gisdata.tucsonaz.gov", search: Search::ArcGis("9coHY2fvuFjG9HQX") },
        ],
    },
    Set {
        name: "California",
        catalogs: &[
            Catalog { name: "Alameda County", url: "https://data.acgov.org", search: Search::ArcGis("ROBnTHSNjoZ2Wm1P") },
            Catalog { name: "Berkeley", url: "https://data.cityofberkeley.info", search: Search::Socrata("data.cityofberkeley.info") },
            Catalog { name: "Fresno", url: "https://gis-cityoffresno.hub.arcgis.com", search: Search::ArcGis("WkBUojyNPhsWOk1W") },
            Catalog {
                name: "Irvine",
                url: "https://city-of-irvine-open-data-portal-cityofirvine.hub.arcgis.com",
                search: Search::ArcGis("3mkVbLdbLBFHrfbK"),
            },
            Catalog { name: "Long Beach", url: "https://datalb.longbeach.gov", search: Search::ArcGis("yCArG7wGXGyWLqav") },
            Catalog {
                name: "Los Angeles County eGIS",
                url: "https://egis-lacounty.hub.arcgis.com",
                search: Search::ArcGis("RmCCgQtiZLDCtblq"),
            },
            Catalog { name: "Los Angeles County Open Data", url: "https://data.lacounty.gov", search: Search::ArcGis("RmCCgQtiZLDCtblq") },
            Catalog { name: "Los Angeles GeoHub", url: "https://geohub.lacity.org", search: Search::ArcGis("7nsPwEMP38bSkCjy") },
            Catalog { name: "Los Angeles Open Data", url: "https://data.lacity.org", search: Search::Socrata("data.lacity.org") },
            Catalog { name: "Marin County", url: "https://data.marincounty.gov", search: Search::Socrata("data.marincounty.gov") },
            Catalog { name: "Oakland", url: "https://data.oaklandca.gov", search: Search::Socrata("data.oaklandca.gov") },
            Catalog { name: "Orange County", url: "https://data-ocpw.opendata.arcgis.com", search: Search::ArcGis("UXmFoWC7yDHcDN5Q") },
            Catalog {
                name: "Riverside County",
                url: "https://gisopendata-countyofriverside.opendata.arcgis.com",
                search: Search::ArcGis("pWmBUdSlVpXStHU6"),
            },
            Catalog { name: "Sacramento", url: "https://data.cityofsacramento.org", search: Search::ArcGis("54falWtcpty3V47Z") },
            Catalog { name: "Sacramento County", url: "https://data.saccounty.gov", search: Search::ArcGis("5NARefyPVtAeuJPU") },
            Catalog {
                name: "San Bernardino County",
                url: "https://open-data-sbcounty.hub.arcgis.com",
                search: Search::ArcGis("aA3snZwJfFkVyDuP"),
            },
            Catalog {
                name: "San Diego County",
                url: "https://data.sandiegocounty.gov",
                search: Search::Socrata("data.sandiegocounty.gov"),
            },
            Catalog { name: "San Francisco", url: "https://data.sf.gov", search: Search::Socrata("data.sf.gov") },
            Catalog { name: "San Jose", url: "https://gisdata-csj.opendata.arcgis.com", search: Search::ArcGis("6kSayNlqm3HvsYZ8") },
            Catalog { name: "San Mateo County", url: "https://data.smcgov.org", search: Search::Socrata("data.smcgov.org") },
            Catalog { name: "Santa Clara County", url: "https://data.sccgov.org", search: Search::Socrata("data.sccgov.org") },
            Catalog { name: "Santa Monica", url: "https://gisdata.santamonica.gov", search: Search::ArcGis("ntb1ZybmOdA9GyKm") },
            Catalog { name: "Sonoma County", url: "https://data.sonomacounty.ca.gov", search: Search::Socrata("data.sonomacounty.ca.gov") },
            Catalog { name: "West Hollywood", url: "https://data.weho.org", search: Search::Socrata("data.weho.org") },
        ],
    },
    Set {
        name: "Colorado",
        catalogs: &[
            Catalog { name: "Aurora", url: "https://data-auroraco.opendata.arcgis.com", search: Search::ArcGis("0Va1ID99NSrNyyPX") },
            Catalog { name: "Boulder", url: "https://open-data.bouldercolorado.gov", search: Search::ArcGis("ePKBjXrBZ2vEEgWd") },
            Catalog { name: "Denver", url: "https://opendata-geospatialdenver.hub.arcgis.com", search: Search::ArcGis("zdB7qR0BtYrg0Xpl") },
            Catalog {
                name: "El Paso County (Colorado Springs)",
                url: "https://opendata-elpasoco.hub.arcgis.com",
                search: Search::ArcGis("r1Gf4AJYRBIM0N25"),
            },
            Catalog { name: "Fort Collins", url: "https://opendata.fcgov.com", search: Search::Socrata("opendata.fcgov.com") },
        ],
    },
    Set {
        name: "Connecticut",
        catalogs: &[Catalog {
            name: "Hartford",
            url: "https://open-data-hartford-hartfordgis.hub.arcgis.com",
            search: Search::ArcGis("WM6ZNcwewSWH8Mo9"),
        }],
    },
    Set {
        name: "Florida",
        catalogs: &[
            Catalog { name: "Broward County", url: "https://geohub-bcgis.opendata.arcgis.com", search: Search::ArcGis("JMAJrTsHNLrSsWf5") },
            Catalog {
                name: "Gainesville",
                url: "https://data.cityofgainesville.org",
                search: Search::Socrata("data.cityofgainesville.org"),
            },
            Catalog { name: "Miami", url: "https://datahub-miamigis.opendata.arcgis.com", search: Search::ArcGis("CvuPhqcTQpZPT9qY") },
            Catalog { name: "Miami-Dade County", url: "https://gis-mdc.opendata.arcgis.com", search: Search::ArcGis("8Pc9XBTAsYuxx9Ny") },
            Catalog { name: "Orlando", url: "https://data.cityoforlando.net", search: Search::Socrata("data.cityoforlando.net") },
            Catalog {
                name: "Palm Beach County",
                url: "https://opendata2-pbcgov.opendata.arcgis.com",
                search: Search::ArcGis("ZWOoUZbtaYePLlPw"),
            },
            Catalog { name: "St. Petersburg", url: "https://geohub-csp.opendata.arcgis.com", search: Search::ArcGis("9qPLjNtocjo438CJ") },
            Catalog {
                name: "Tallahassee-Leon County",
                url: "https://geodata-tlcgis.opendata.arcgis.com",
                search: Search::ArcGis("ptvDyBs1KkcwzQNJ"),
            },
            Catalog { name: "Tampa", url: "https://city-tampa.opendata.arcgis.com", search: Search::ArcGis("IbNXlmt2RVVRCZ6M") },
        ],
    },
    Set {
        name: "Georgia",
        catalogs: &[
            Catalog {
                name: "Athens-Clarke County",
                url: "https://data-athensclarke.opendata.arcgis.com",
                search: Search::ArcGis("xSEULKvB31odt3XQ"),
            },
            Catalog {
                name: "Atlanta Regional Commission",
                url: "https://opendata.atlantaregional.com",
                search: Search::ArcGis("Ug5xGQbHsD8zuZzM"),
            },
            Catalog { name: "DeKalb County", url: "https://dcgis-dekalbgis.hub.arcgis.com", search: Search::ArcGis("IxVN2oUE9EYLSnPE") },
            Catalog {
                name: "Gwinnett County",
                url: "https://gcgis-gwinnettcountyga.hub.arcgis.com",
                search: Search::ArcGis("RfpmnkSAQleRbndX"),
            },
        ],
    },
    Set {
        name: "Hawaii",
        catalogs: &[
            Catalog { name: "Honolulu GIS", url: "https://honolulu-cchnl.opendata.arcgis.com", search: Search::ArcGis("tNJpAOha4mODLkXz") },
            Catalog { name: "Honolulu Open Data", url: "https://data.honolulu.gov", search: Search::Socrata("data.honolulu.gov") },
        ],
    },
    Set {
        name: "Idaho",
        catalogs: &[Catalog { name: "Boise", url: "https://opendata.cityofboise.org", search: Search::ArcGis("WHM6qC35aMtyAAlN") }],
    },
    Set {
        name: "Illinois",
        catalogs: &[
            Catalog { name: "Chicago", url: "https://data.cityofchicago.org", search: Search::Socrata("data.cityofchicago.org") },
            Catalog {
                name: "Cook County GIS",
                url: "https://hub-cookcountyil.opendata.arcgis.com",
                search: Search::ArcGis("I5Or36sMcO7Y9vQ3"),
            },
            Catalog {
                name: "Cook County Open Data",
                url: "https://datacatalog.cookcountyil.gov",
                search: Search::Socrata("datacatalog.cookcountyil.gov"),
            },
            Catalog { name: "Naperville", url: "https://data.naperville.il.us", search: Search::ArcGis("rXJ6QApc2sOtl1Pd") },
        ],
    },
    Set {
        name: "Indiana",
        catalogs: &[
            Catalog { name: "Bloomington", url: "https://data.bloomington.in.gov", search: Search::Socrata("data.bloomington.in.gov") },
            Catalog { name: "Hamilton County", url: "https://geohub.hamiltoncounty.in.gov", search: Search::ArcGis("beYj0ONLvCt8qxHA") },
            Catalog { name: "Indianapolis", url: "https://data.indy.gov", search: Search::ArcGis("xBsPUWYKO89lShIO") },
        ],
    },
    Set {
        name: "Kansas",
        catalogs: &[
            Catalog { name: "Johnson County", url: "https://jocogov-aims.opendata.arcgis.com", search: Search::ArcGis("VI7SIDfzMs53TTMm") },
            Catalog { name: "Olathe", url: "https://data-cityofolathe.opendata.arcgis.com", search: Search::ArcGis("V5kJ9QPOiI1K62Ac") },
            Catalog {
                name: "Sedgwick County",
                url: "https://sedgwick-county-gis-sedgwickcounty.hub.arcgis.com",
                search: Search::ArcGis("McLat6HlPl45bNBv"),
            },
            Catalog { name: "Wichita", url: "https://data-cityofwichita.hub.arcgis.com", search: Search::ArcGis("lOHEurd1BgncOSk1") },
        ],
    },
    Set {
        name: "Kentucky",
        catalogs: &[
            Catalog { name: "Lexington", url: "https://data-lfucg.hub.arcgis.com", search: Search::ArcGis("Mg7DLdfYcSWIaDnu") },
            Catalog { name: "Louisville", url: "https://data.louisvilleky.gov", search: Search::ArcGis("79kfd2K6fskCAkyg") },
        ],
    },
    Set {
        name: "Louisiana",
        catalogs: &[
            Catalog { name: "Baton Rouge", url: "https://data.brla.gov", search: Search::Socrata("data.brla.gov") },
            Catalog { name: "New Orleans", url: "https://data.nola.gov", search: Search::Socrata("data.nola.gov") },
        ],
    },
    Set {
        name: "Maryland",
        catalogs: &[
            Catalog { name: "Baltimore", url: "https://data.baltimorecity.gov", search: Search::ArcGis("UWYHeuuJISiGmgXx") },
            Catalog { name: "Baltimore County", url: "https://opendata.baltimorecountymd.gov", search: Search::ArcGis("Ynpzre7M7vSGY7dh") },
            Catalog {
                name: "Montgomery County",
                url: "https://data.montgomerycountymd.gov",
                search: Search::Socrata("data.montgomerycountymd.gov"),
            },
            Catalog {
                name: "Prince George's County",
                url: "https://data.princegeorgescountymd.gov",
                search: Search::Socrata("data.princegeorgescountymd.gov"),
            },
        ],
    },
    Set {
        name: "Massachusetts",
        catalogs: &[
            Catalog {
                name: "Boston",
                url: "https://bostonopendata-boston.opendata.arcgis.com",
                search: Search::ArcGis("sFnw0xNflSi8J0uh"),
            },
            Catalog { name: "Cambridge", url: "https://data.cambridgema.gov", search: Search::Socrata("data.cambridgema.gov") },
        ],
    },
    Set {
        name: "Michigan",
        catalogs: &[Catalog { name: "Detroit", url: "https://data.detroitmi.gov", search: Search::ArcGis("qvkbeam7Wirps6zC") }],
    },
    Set {
        name: "Minnesota",
        catalogs: &[
            Catalog { name: "Hennepin County", url: "https://gis-hennepin.hub.arcgis.com", search: Search::ArcGis("ziLNoRgqnICUM0q1") },
            Catalog { name: "Minneapolis", url: "https://opendata.minneapolismn.gov", search: Search::ArcGis("afSMGVsC7QlRK1kZ") },
            Catalog { name: "St. Paul", url: "https://information.stpaul.gov", search: Search::ArcGis("9meaaHE3uiba0zr8") },
        ],
    },
    Set {
        name: "Missouri",
        catalogs: &[
            Catalog {
                name: "Columbia",
                url: "https://datahub-gocolumbiamo.opendata.arcgis.com",
                search: Search::ArcGis("GHhNHT1xiCkCAXvo"),
            },
            Catalog { name: "Kansas City", url: "https://data.kcmo.org", search: Search::Socrata("data.kcmo.org") },
        ],
    },
    Set {
        name: "Nebraska",
        catalogs: &[Catalog {
            name: "Omaha-Douglas County",
            url: "https://data-dogis.opendata.arcgis.com",
            search: Search::ArcGis("pDAi2YK0L0QxVJHj"),
        }],
    },
    Set {
        name: "Nevada",
        catalogs: &[
            Catalog {
                name: "Las Vegas",
                url: "https://opendataportal-lasvegas.opendata.arcgis.com",
                search: Search::ArcGis("F1v0ufATbBQScMtY"),
            },
            Catalog { name: "Reno", url: "https://data-cityofreno.opendata.arcgis.com", search: Search::ArcGis("RjM4BJrv5ZkqP4XC") },
        ],
    },
    Set {
        name: "New York",
        catalogs: &[
            Catalog { name: "Buffalo", url: "https://data.buffalony.gov", search: Search::Socrata("data.buffalony.gov") },
            Catalog { name: "New York City", url: "https://data.cityofnewyork.us", search: Search::Socrata("data.cityofnewyork.us") },
            Catalog { name: "Rochester", url: "https://data.cityofrochester.gov", search: Search::ArcGis("yoz1ZtATTCokO9nU") },
            Catalog { name: "Suffolk County", url: "https://opendata.suffolkcountyny.gov", search: Search::ArcGis("JsDD4qdG5r2a7hR5") },
            Catalog { name: "Syracuse", url: "https://data.syr.gov", search: Search::ArcGis("bdPqSfflsdgFRVVM") },
            Catalog { name: "Westchester County", url: "https://gis.westchestercountyny.gov", search: Search::ArcGis("XKEHpOulfycN9cGC") },
        ],
    },
    Set {
        name: "North Carolina",
        catalogs: &[
            Catalog { name: "Charlotte", url: "https://data.charlottenc.gov", search: Search::ArcGis("9Nl857LBlQVyzq54") },
            Catalog { name: "Durham", url: "https://live-durhamnc.opendata.arcgis.com", search: Search::ArcGis("G5vR3cOjh6g2Ed8E") },
            Catalog { name: "Greensboro", url: "https://data.greensboro-nc.gov", search: Search::ArcGis("A7KFW0gHh8qBaXk3") },
            Catalog { name: "Raleigh", url: "https://data-ral.opendata.arcgis.com", search: Search::ArcGis("v400IkDOw1ad7Yad") },
            Catalog { name: "Wake County", url: "https://data-wake.opendata.arcgis.com", search: Search::ArcGis("a7CWfuGP5ZnLYE7I") },
        ],
    },
    Set {
        name: "Ohio",
        catalogs: &[
            Catalog { name: "Cleveland", url: "https://data.clevelandohio.gov", search: Search::ArcGis("dty2kHktVXHrqO8i") },
            Catalog { name: "Columbus", url: "https://opendata.columbus.gov", search: Search::ArcGis("9yy6msODkIBzkUXU") },
            Catalog {
                name: "Cuyahoga County",
                url: "https://geospatial.gis.cuyahogacounty.gov",
                search: Search::ArcGis("GXM8JipKyc0m6HBi"),
            },
            Catalog { name: "Toledo", url: "https://data.toledo.gov", search: Search::ArcGis("2snQ88YUjP9CNEbe") },
        ],
    },
    Set {
        name: "Oklahoma",
        catalogs: &[
            Catalog { name: "Oklahoma City", url: "https://data.okc.gov", search: Search::ArcGis("2mOVdIcRtNH2JsSF") },
            Catalog { name: "Tulsa", url: "https://gis2-cityoftulsa.opendata.arcgis.com", search: Search::ArcGis("XkZ90iCdbTJ9oNXl") },
        ],
    },
    Set {
        name: "Oregon",
        catalogs: &[
            Catalog { name: "Jackson County", url: "https://gis-jcgis.opendata.arcgis.com", search: Search::ArcGis("DwYBkWQPdaJNWrPG") },
            Catalog {
                name: "Oregon Metro (Portland region)",
                url: "https://rlisdiscovery.oregonmetro.gov",
                search: Search::ArcGis("McQ0OlIABe29rJJy"),
            },
            Catalog { name: "Portland", url: "https://gis-pdx.opendata.arcgis.com", search: Search::ArcGis("quVN97tn06YNGj9s") },
        ],
    },
    Set {
        name: "Pennsylvania",
        catalogs: &[
            Catalog { name: "Philadelphia", url: "https://data-phl.opendata.arcgis.com", search: Search::ArcGis("fLeGjb7u4uXqeF9q") },
            Catalog {
                name: "Pittsburgh",
                url: "https://pghgishub-pittsburghpa.opendata.arcgis.com",
                search: Search::ArcGis("YZCmUqbcsUpOKfj7"),
            },
        ],
    },
    Set {
        name: "Rhode Island",
        catalogs: &[
            Catalog {
                name: "Providence GIS",
                url: "https://providence-gis-hub-pvdgis.hub.arcgis.com",
                search: Search::ArcGis("wv9mHoqblhTsnqdG"),
            },
            Catalog {
                name: "Providence Open Data",
                url: "https://data.providenceri.gov",
                search: Search::Socrata("data.providenceri.gov"),
            },
        ],
    },
    Set {
        name: "South Carolina",
        catalogs: &[Catalog {
            name: "Charleston County",
            url: "https://charleston-county-gis-chascogis.hub.arcgis.com",
            search: Search::ArcGis("jR9eNCjAkxwH2nLe"),
        }],
    },
    Set {
        name: "Tennessee",
        catalogs: &[
            Catalog {
                name: "Memphis",
                url: "https://memphis-open-data-hub-memegis.hub.arcgis.com",
                search: Search::ArcGis("saWmpKJIUAjyyNVc"),
            },
            Catalog { name: "Nashville", url: "https://data.nashville.gov", search: Search::ArcGis("HdTo6HJqh92wn4D8") },
        ],
    },
    Set {
        name: "Texas",
        catalogs: &[
            Catalog { name: "Austin", url: "https://data.austintexas.gov", search: Search::Socrata("data.austintexas.gov") },
            Catalog { name: "Bexar County", url: "https://gis-bexar.opendata.arcgis.com", search: Search::ArcGis("8onVmslF2KXErTHT") },
            Catalog { name: "Corpus Christi", url: "https://gis-corpus.opendata.arcgis.com", search: Search::ArcGis("0J4ZNc4NaTguvRy0") },
            Catalog {
                name: "Dallas County",
                url: "https://dallas-county-open-data-hub-dallascountygis.hub.arcgis.com",
                search: Search::ArcGis("zqe2kwz79KUqUvxC"),
            },
            Catalog { name: "Dallas GIS", url: "https://egisdata-dallasgis.hub.arcgis.com", search: Search::ArcGis("rwnOSbfKSwyTBcwN") },
            Catalog { name: "Dallas Open Data", url: "https://www.dallasopendata.com", search: Search::Socrata("www.dallasopendata.com") },
            Catalog { name: "El Paso", url: "https://opendata.elpasotexas.gov", search: Search::ArcGis("hyTVSIhR7dHyDsJF") },
            Catalog { name: "Fort Worth", url: "https://data.fortworthtexas.gov", search: Search::ArcGis("3ddLCBXe1bRt7mzj") },
            Catalog {
                name: "Harris County",
                url: "https://geo-harriscounty.opendata.arcgis.com",
                search: Search::ArcGis("su8ic9KbA7PYVxPS"),
            },
            Catalog { name: "Houston", url: "https://cohgis-mycity.opendata.arcgis.com", search: Search::ArcGis("NummVBqZSIJKUeVR") },
            Catalog {
                name: "San Antonio",
                url: "https://opendata-cosagis.opendata.arcgis.com",
                search: Search::ArcGis("g1fRTDLeMgspWrYp"),
            },
            Catalog {
                name: "Tarrant County",
                url: "https://data-tarrantcounty.opendata.arcgis.com",
                search: Search::ArcGis("KgTkwJ7lfG2eHF3z"),
            },
            Catalog { name: "Travis County", url: "https://tnr-traviscountytx.hub.arcgis.com", search: Search::ArcGis("HGcSYZ5bvjRswoCb") },
        ],
    },
    Set {
        name: "Utah",
        catalogs: &[
            Catalog { name: "Salt Lake City", url: "https://gis-slcgov.opendata.arcgis.com", search: Search::ArcGis("mMBpeYj0vPFotzbe") },
            Catalog {
                name: "Salt Lake County",
                url: "https://gisdata-slco.opendata.arcgis.com",
                search: Search::ArcGis("DJP723NX3ukQ2LtF"),
            },
        ],
    },
    Set {
        name: "Virginia",
        catalogs: &[
            Catalog {
                name: "Arlington County",
                url: "https://gisdata-arlgis.opendata.arcgis.com",
                search: Search::ArcGis("JrtUzVcH8S2wnzVH"),
            },
            Catalog { name: "Chesapeake", url: "https://public-chesva.opendata.arcgis.com", search: Search::ArcGis("OAphVRUvJVpBRovs") },
            Catalog {
                name: "Fairfax County",
                url: "https://data-fairfaxcountygis.opendata.arcgis.com",
                search: Search::ArcGis("ioennV6PpG5Xodq0"),
            },
            Catalog {
                name: "Loudoun County",
                url: "https://geohub-loudoungis.opendata.arcgis.com",
                search: Search::ArcGis("MxjRokvPm7bjslyR"),
            },
            Catalog { name: "Norfolk", url: "https://data.norfolk.gov", search: Search::Socrata("data.norfolk.gov") },
            Catalog { name: "Richmond", url: "https://data.richmondgov.com", search: Search::Socrata("data.richmondgov.com") },
            Catalog { name: "Virginia Beach", url: "https://gis.data.vbgov.com", search: Search::ArcGis("CyVvlIiUfRBmMQuu") },
        ],
    },
    Set {
        name: "Washington",
        catalogs: &[
            Catalog {
                name: "Clark County",
                url: "https://hub-clarkcountywa.opendata.arcgis.com",
                search: Search::ArcGis("ylxwjFBdCPBzP16d"),
            },
            Catalog {
                name: "King County GIS",
                url: "https://gis-kingcounty.opendata.arcgis.com",
                search: Search::ArcGis("Ej0PsM5Aw677QF1W"),
            },
            Catalog { name: "King County Open Data", url: "https://data.kingcounty.gov", search: Search::Socrata("data.kingcounty.gov") },
            Catalog {
                name: "Pierce County",
                url: "https://gisdata-piercecowa.opendata.arcgis.com",
                search: Search::ArcGis("1UvBaQ5y1ubjUPmd"),
            },
            Catalog {
                name: "Seattle GeoData",
                url: "https://data-seattlecitygis.opendata.arcgis.com",
                search: Search::ArcGis("ZOyb2t4B0UYuYNYH"),
            },
            Catalog { name: "Seattle Open Data", url: "https://data.seattle.gov", search: Search::Socrata("data.seattle.gov") },
            Catalog {
                name: "Snohomish County",
                url: "https://snohomish-county-open-data-portal-snoco-gis.hub.arcgis.com",
                search: Search::ArcGis("z6WYi9VRHfgwgtyW"),
            },
            Catalog { name: "Spokane", url: "https://data-spokane.opendata.arcgis.com", search: Search::ArcGis("3PDwyTturHqnGCu0") },
            Catalog { name: "Tacoma", url: "https://data.tacoma.gov", search: Search::ArcGis("SCwJH1pD8WSn5T5y") },
        ],
    },
    Set {
        name: "Wisconsin",
        catalogs: &[
            Catalog { name: "Madison", url: "https://data-cityofmadison.opendata.arcgis.com", search: Search::ArcGis("lx96Ahunbwmk5g5p") },
            Catalog { name: "Milwaukee County", url: "https://data.county.milwaukee.gov", search: Search::ArcGis("s1wgJQKbKJihhhaT") },
        ],
    },
    Set {
        name: "Wyoming",
        catalogs: &[Catalog {
            name: "Cheyenne-Laramie County",
            url: "https://hub.clcgisc.com",
            search: Search::ArcGis("e5t4ywWgRBNUuDeU"),
        }],
    },
];

/// Tennessee's own GIS portal (its GeoData hub), searched alone (also among [`STATE`]'s).
pub const TENNESSEE: &[Set] = &[Set {
    name: "Tennessee",
    catalogs: &[Catalog { name: "Tennessee GeoData", url: "https://geodata.tn.gov", search: Search::ArcGis("YuVBSS7Y1of2Qud1") }],
}];
