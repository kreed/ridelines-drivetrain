use geojson::{Feature, FeatureCollection, GeoJson, Geometry, Value};
use gpx::{Gpx, read};
use indicatif::{ProgressBar, ProgressStyle};
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use geo::{Point, HaversineDistance};
use futures::stream::{self, StreamExt};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

pub async fn convert_gpx_directory(directory: &Path) {
    println!("Converting GPX files in directory: {}", directory.display());

    // Find all GPX files in the directory
    let gpx_files = match discover_gpx_files(directory) {
        Ok(files) => files,
        Err(e) => {
            eprintln!("Error discovering GPX files: {e}");
            return;
        }
    };

    if gpx_files.is_empty() {
        println!("No GPX files found in directory: {}", directory.display());
    } else {
        println!("Found {} GPX files to process", gpx_files.len());
    }

    // Filter out GPX files that already have corresponding GeoJSON files
    let files_to_convert: Vec<_> = gpx_files
        .iter()
        .filter(|gpx_file| {
            let geojson_path = gpx_file.with_extension("geojson");
            !geojson_path.exists()
        })
        .collect();

    let skipped_count = gpx_files.len() - files_to_convert.len();
    if skipped_count > 0 {
        println!("Skipping {skipped_count} GPX files that already have GeoJSON files");
    }

    // Clean up orphaned GeoJSON files
    let deleted_count = match cleanup_orphaned_geojson_files(directory) {
        Ok(count) => count,
        Err(e) => {
            eprintln!("Error cleaning up orphaned GeoJSON files: {e}");
            0
        }
    };

    if deleted_count > 0 {
        println!("Deleted {deleted_count} orphaned GeoJSON files");
    }

    if files_to_convert.is_empty() {
        println!("No GPX files need conversion");
        return;
    }

    println!("Converting {} GPX files", files_to_convert.len());

    // Set up progress bar
    let pb = ProgressBar::new(files_to_convert.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );

    let converted_count = Arc::new(Mutex::new(0));
    let error_count = Arc::new(Mutex::new(0));
    
    // Limit concurrent conversions to avoid overwhelming the system
    let semaphore = Arc::new(Semaphore::new(4));
    let pb = Arc::new(pb);

    // Process files in parallel
    stream::iter(files_to_convert)
        .map(|gpx_file| {
            let semaphore = semaphore.clone();
            let pb = pb.clone();
            let converted_count = converted_count.clone();
            let error_count = error_count.clone();

            async move {
                let _permit = semaphore.acquire().await.unwrap();
                
                pb.set_message(format!(
                    "Converting {}",
                    gpx_file.file_name().unwrap().to_string_lossy()
                ));

                match convert_gpx_to_geojson(gpx_file).await {
                    Ok(_) => {
                        if let Ok(mut count) = converted_count.lock() {
                            *count += 1;
                        }
                    }
                    Err(e) => {
                        eprintln!("Error converting {}: {}", gpx_file.display(), e);
                        if let Ok(mut count) = error_count.lock() {
                            *count += 1;
                        }
                    }
                }

                pb.inc(1);
            }
        })
        .buffer_unordered(4)
        .collect::<Vec<_>>()
        .await;

    pb.finish_with_message("Conversion complete!");

    let final_converted_count = *converted_count.lock().unwrap();
    let final_error_count = *error_count.lock().unwrap();
    
    println!("Conversion summary:");
    println!("  Converted: {final_converted_count}");
    println!("  Skipped (already exists): {skipped_count}");
    println!("  Deleted (orphaned): {deleted_count}");
    println!("  Errors: {final_error_count}");
}

fn discover_gpx_files(directory: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut gpx_files = Vec::new();

    if !directory.exists() {
        return Err(format!("Directory does not exist: {}", directory.display()).into());
    }

    if !directory.is_dir() {
        return Err(format!("Path is not a directory: {}", directory.display()).into());
    }

    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() {
            if let Some(extension) = path.extension() {
                if extension.to_string_lossy().to_lowercase() == "gpx" {
                    gpx_files.push(path);
                }
            }
        }
    }

    // Sort files for consistent processing order
    gpx_files.sort();

    Ok(gpx_files)
}

fn cleanup_orphaned_geojson_files(directory: &Path) -> Result<usize, Box<dyn std::error::Error>> {
    let mut deleted_count = 0;

    if !directory.exists() || !directory.is_dir() {
        return Ok(0);
    }

    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() {
            if let Some(extension) = path.extension() {
                if extension.to_string_lossy().to_lowercase() == "geojson" {
                    // Check if corresponding GPX file exists
                    let gpx_path = path.with_extension("gpx");
                    if !gpx_path.exists() {
                        // Delete orphaned GeoJSON file
                        match fs::remove_file(&path) {
                            Ok(_) => deleted_count += 1,
                            Err(e) => eprintln!("Failed to delete orphaned GeoJSON file {}: {}", path.display(), e),
                        }
                    }
                }
            }
        }
    }

    Ok(deleted_count)
}

fn split_segment_on_gaps(points: &[gpx::Waypoint], max_gap_meters: f64) -> Vec<Vec<Vec<f64>>> {
    let mut linestrings = Vec::new();
    let mut current_coords = Vec::new();
    
    for (i, point) in points.iter().enumerate() {
        let coord = {
            let mut coord = vec![point.point().x(), point.point().y()];
            if let Some(elevation) = point.elevation {
                coord.push(elevation);
            }
            coord
        };
        
        if i == 0 {
            // First point
            current_coords.push(coord);
        } else {
            // Calculate distance from previous point
            let prev_point = &points[i - 1];
            let curr_geo_point = Point::new(point.point().x(), point.point().y());
            let prev_geo_point = Point::new(prev_point.point().x(), prev_point.point().y());
            
            let distance = curr_geo_point.haversine_distance(&prev_geo_point);
            
            if distance > max_gap_meters {
                // Gap is too large, start a new linestring
                if current_coords.len() > 1 {
                    linestrings.push(current_coords);
                }
                current_coords = vec![coord];
            } else {
                // Normal distance, continue current linestring
                current_coords.push(coord);
            }
        }
    }
    
    // Add the final linestring if it has more than one point
    if current_coords.len() > 1 {
        linestrings.push(current_coords);
    }
    
    // If no valid linestrings were created but we have points, 
    // return a single linestring with all points
    if linestrings.is_empty() && !points.is_empty() {
        let coords: Vec<Vec<f64>> = points
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
            linestrings.push(coords);
        }
    }
    
    linestrings
}

async fn convert_gpx_to_geojson(gpx_file: &Path) -> Result<(), Box<dyn std::error::Error>> {
    // Read and parse GPX file
    let file = fs::File::open(gpx_file)?;
    let reader = BufReader::new(file);
    let gpx: Gpx = read(reader)?;

    // Create GeoJSON FeatureCollection
    let mut features = Vec::new();

    // Convert tracks only
    for track in &gpx.tracks {
        for segment in &track.segments {
            if !segment.points.is_empty() {
                // Split segment into multiple linestrings if gaps > 100m exist
                let linestrings = split_segment_on_gaps(&segment.points, 100.0);
                
                for (i, coords) in linestrings.into_iter().enumerate() {
                    let geometry = Geometry::new(Value::LineString(coords));

                    let mut properties = serde_json::Map::new();
                    if let Some(name) = &track.name {
                        let segment_name = if i == 0 {
                            name.clone()
                        } else {
                            format!("{}_part_{}", name, i + 1)
                        };
                        properties.insert("name".to_string(), serde_json::Value::String(segment_name));
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

    // Create output file path (same name but with .geojson extension)
    let output_path = gpx_file.with_extension("geojson");

    // Write GeoJSON to file
    let geojson_string =
        serde_json::to_string_pretty(&GeoJson::FeatureCollection(feature_collection))?;
    fs::write(&output_path, geojson_string)?;

    Ok(())
}
