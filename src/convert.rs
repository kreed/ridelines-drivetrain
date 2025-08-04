use geojson::{Feature, FeatureCollection, GeoJson, Geometry, Value};
use gpx::{Gpx, read};
use indicatif::{ProgressBar, ProgressStyle};
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};

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

    let mut converted_count = 0;
    let mut error_count = 0;

    for gpx_file in files_to_convert {
        pb.set_message(format!(
            "Converting {}",
            gpx_file.file_name().unwrap().to_string_lossy()
        ));

        match convert_gpx_to_geojson(gpx_file).await {
            Ok(_) => {
                converted_count += 1;
            }
            Err(e) => {
                eprintln!("Error converting {}: {}", gpx_file.display(), e);
                error_count += 1;
            }
        }

        pb.inc(1);
    }

    pb.finish_with_message("Conversion complete!");

    println!("Conversion summary:");
    println!("  Converted: {converted_count}");
    println!("  Skipped (already exists): {skipped_count}");
    println!("  Deleted (orphaned): {deleted_count}");
    println!("  Errors: {error_count}");
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
                // Create LineString from track points
                let coords: Vec<Vec<f64>> = segment
                    .points
                    .iter()
                    .map(|point| {
                        let mut coord = vec![point.point().x(), point.point().y()];
                        if let Some(elevation) = point.elevation {
                            coord.push(elevation);
                        }
                        coord
                    })
                    .collect();

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
