use geojson::{Feature, FeatureCollection, GeoJson, Geometry, Value};
use gpx::{Gpx, read};
use std::io::BufReader;

pub async fn convert_gpx_to_geojson(gpx_data: &str) -> Result<String, Box<dyn std::error::Error>> {
    // Parse GPX data
    let reader = BufReader::new(gpx_data.as_bytes());
    let gpx: Gpx = read(reader)?;

    // Create GeoJSON FeatureCollection
    let mut features = Vec::new();

    // Convert tracks only
    for track in &gpx.tracks {
        for segment in &track.segments {
            if !segment.points.is_empty() {
                // Convert segment points to coordinates
                let coords: Vec<Vec<f64>> = segment.points
                    .iter()
                    .map(|point| {
                        let mut coord = vec![point.point().x(), point.point().y()];
                        if let Some(elevation) = point.elevation {
                            coord.push(elevation);
                        }
                        coord
                    })
                    .collect();

                if coords.len() > 1 {
                    let geometry = Geometry::new(Value::LineString(coords));

                    let mut properties = serde_json::Map::new();
                    if let Some(name) = &track.name {
                        properties.insert("name".to_string(), serde_json::Value::String(name.clone()));
                    }
                    properties.insert(
                        "type".to_string(),
                        serde_json::Value::String("track".to_string()),
                    );

                    let feature = Feature {
                        bbox: None,
                        geometry: Some(geometry),
                        id: None,
                        properties: Some(properties),
                        foreign_members: None,
                    };

                    features.push(feature);
                }
            }
        }
    }

    // Create FeatureCollection
    let feature_collection = FeatureCollection {
        bbox: None,
        features,
        foreign_members: None,
    };

    // Convert to GeoJSON string
    let geojson_string =
        serde_json::to_string_pretty(&GeoJson::FeatureCollection(feature_collection))?;

    Ok(geojson_string)
}
