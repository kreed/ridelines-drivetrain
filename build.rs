use std::env;
use std::fs;
use std::path::Path;

fn main() {
    // Tell cargo to rerun if the OpenAPI spec changes
    println!("cargo:rerun-if-changed=openapi/ridelines-api.yaml");

    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("generated_types.rs");

    // Read the OpenAPI spec
    let openapi_content = fs::read_to_string("openapi/ridelines-api.yaml")
        .expect("Failed to read OpenAPI specification");

    // Parse the OpenAPI spec
    let openapi_spec: serde_yaml::Value = serde_yaml::from_str(&openapi_content)
        .expect("Failed to parse OpenAPI specification");

    // Extract the components/schemas section
    let schemas = openapi_spec
        .get("components")
        .and_then(|c| c.get("schemas"))
        .expect("No schemas found in OpenAPI specification");

    // Convert to JSON for typify (typify expects JSON Schema)
    let schemas_json = serde_json::to_value(schemas)
        .expect("Failed to convert schemas to JSON");

    // Generate Rust types using typify
    // Create a complete JSON Schema document for proper $ref resolution  
    let json_schema_document = serde_json::json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "definitions": schemas_json
    });

    // Use typify to generate the code
    let mut settings = typify::TypeSpaceSettings::default();
    settings.with_struct_builder(true);

    let mut type_space = typify::TypeSpace::new(&settings);
    
    // Convert to schema and add
    let schema: schemars::schema::RootSchema = serde_json::from_value(json_schema_document)
        .expect("Failed to parse schema document");
    
    // Add the reference types from definitions
    if !schema.definitions.is_empty() {
        type_space.add_ref_types(schema.definitions)
            .expect("Failed to add reference types");
    }

    // Generate the Rust code
    let generated_code = type_space.to_stream().to_string();

    // Write the generated types to the output file
    fs::write(&dest_path, generated_code)
        .expect("Failed to write generated types");

    println!("Generated API types at: {}", dest_path.display());
}