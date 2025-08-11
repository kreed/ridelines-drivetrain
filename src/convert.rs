use geojson::{Feature, FeatureCollection, GeoJson, Geometry, Value};
use fitparser::{profile::MesgNum, FitDataRecord, Value as FitValue};


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

pub async fn convert_fit_to_geojson(fit_data: &[u8]) -> Result<Option<String>, Box<dyn std::error::Error>> {
    // Parse FIT data
    let fit_data_records = fitparser::from_bytes(fit_data)?;
    
    // Extract GPS coordinates from record messages
    let mut coords: Vec<Vec<f64>> = Vec::new();
    
    for data_record in fit_data_records {
        if data_record.kind() == MesgNum::Record {
            if let Some(coord) = extract_coordinate_from_record(&data_record) {
                coords.push(coord);
            }
        }
    }
    
    // Return None if no coordinates found
    if coords.len() <= 1 {
        return Ok(None);
    }
    
    // Create GeoJSON FeatureCollection
    let mut features = Vec::new();
    
    let geometry = Geometry::new(Value::LineString(coords));
    
    let mut properties = serde_json::Map::new();
    properties.insert("name".to_string(), serde_json::Value::String("FIT Track".to_string()));
    properties.insert("type".to_string(), serde_json::Value::String("track".to_string()));
    
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
    
    // Convert to GeoJSON string
    let geojson_string = serde_json::to_string_pretty(&GeoJson::FeatureCollection(feature_collection))?;
    
    Ok(Some(geojson_string))
}
