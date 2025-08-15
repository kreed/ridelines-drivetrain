use fitparser::{FitDataRecord, Value as FitValue, profile::MesgNum};
use geo::{Distance, Haversine, point};
use geojson::{Feature, FeatureCollection, GeoJson, Geometry, Value};

fn extract_coordinate_from_record(data_record: &FitDataRecord) -> Option<Vec<f64>> {
    let fields = data_record.fields();

    let mut lat_opt = None;
    let mut lon_opt = None;
    let mut alt_opt = None;

    // Extract latitude, longitude, and altitude
    for field in fields {
        match field.name() {
            "position_lat" => {
                if let FitValue::SInt32(lat_semicircles) = field.value() {
                    // Convert from semicircles to degrees
                    lat_opt = Some(*lat_semicircles as f64 * (180.0 / 2_147_483_648.0));
                }
            }
            "position_long" => {
                if let FitValue::SInt32(lon_semicircles) = field.value() {
                    // Convert from semicircles to degrees
                    lon_opt = Some(*lon_semicircles as f64 * (180.0 / 2_147_483_648.0));
                }
            }
            "altitude" => {
                if let FitValue::UInt16(alt_mm) = field.value() {
                    // Convert from mm to meters (subtract 500m offset)
                    alt_opt = Some((*alt_mm as f64 / 5.0) - 500.0);
                }
            }
            _ => {}
        }
    }

    // If we have valid lat/lon, create coordinate
    if let (Some(lat), Some(lon)) = (lat_opt, lon_opt) {
        let mut coord = vec![lon, lat]; // GeoJSON uses [lon, lat] order
        if let Some(alt) = alt_opt {
            coord.push(alt);
        }
        Some(coord)
    } else {
        None
    }
}

fn split_coordinates_on_gaps(coords: Vec<Vec<f64>>, max_gap_meters: f64) -> Vec<Vec<Vec<f64>>> {
    if coords.len() <= 1 {
        return vec![coords];
    }

    let mut segments = Vec::new();
    let mut current_segment = Vec::new();

    for (i, coord) in coords.iter().enumerate() {
        current_segment.push(coord.clone());

        // Check gap to next point (if it exists)
        if let Some(next_coord) = coords.get(i + 1) {
            // Create geo::Point objects for distance calculation
            let current_point = point!(x: coord[0], y: coord[1]); // lon, lat
            let next_point = point!(x: next_coord[0], y: next_coord[1]);

            // Calculate distance in meters
            let distance_meters = Haversine.distance(current_point, next_point);

            // If gap is too large, start a new segment
            if distance_meters > max_gap_meters {
                // Only add segment if it has at least 2 points
                if current_segment.len() >= 2 {
                    segments.push(current_segment);
                }
                current_segment = Vec::new();
            }
        }
    }

    // Add the final segment if it has at least 2 points
    if current_segment.len() >= 2 {
        segments.push(current_segment);
    }

    segments
}

pub async fn convert_fit_to_geojson(
    fit_data: &[u8],
    activity: &crate::intervals_client::Activity,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    // Parse FIT data
    let fit_data_records = fitparser::from_bytes(fit_data)?;

    // Extract GPS coordinates from record messages
    let mut coords: Vec<Vec<f64>> = Vec::new();

    for data_record in fit_data_records {
        if data_record.kind() == MesgNum::Record
            && let Some(coord) = extract_coordinate_from_record(&data_record)
        {
            coords.push(coord);
        }
    }

    // Return None if no coordinates found
    if coords.len() <= 1 {
        return Ok(None);
    }

    // Split coordinates on gaps larger than 100m
    let segments = split_coordinates_on_gaps(coords, 100.0);

    // Return None if no valid segments after splitting
    if segments.is_empty() {
        return Ok(None);
    }

    // Create GeoJSON FeatureCollection with a single feature containing MultiLineString
    let mut features = Vec::new();

    let geometry = if segments.len() == 1 {
        Geometry::new(Value::LineString(segments[0].clone()))
    } else {
        Geometry::new(Value::MultiLineString(segments))
    };

    let mut properties = serde_json::Map::new();
    properties.insert(
        "name".to_string(),
        serde_json::Value::String(activity.name.clone()),
    );
    properties.insert(
        "date".to_string(),
        serde_json::Value::String(activity.start_date_local.clone()),
    );
    properties.insert(
        "type".to_string(),
        serde_json::Value::String(activity.activity_type.clone()),
    );
    properties.insert(
        "id".to_string(),
        serde_json::Value::String(activity.id.clone()),
    );

    let feature = Feature {
        bbox: None,
        geometry: Some(geometry),
        id: None,
        properties: Some(properties),
        foreign_members: None,
    };

    features.push(feature);

    // Create FeatureCollection
    let feature_collection = FeatureCollection {
        bbox: None,
        features,
        foreign_members: None,
    };

    // Convert to GeoJSON string (compact format for smaller size)
    let geojson_string =
        serde_json::to_string(&GeoJson::FeatureCollection(feature_collection))?;

    Ok(Some(geojson_string))
}
