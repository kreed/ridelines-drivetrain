mod convert;
mod intervals_client;
mod sync;

#[cfg(not(feature = "lambda"))]
mod cli;

#[cfg(feature = "lambda")]
mod lambda_handler;

#[cfg(not(feature = "lambda"))]
#[tokio::main]
async fn main() {
    cli::cli_main().await;
}

#[cfg(feature = "lambda")]
#[tokio::main] 
async fn main() -> Result<(), lambda_runtime::Error> {
    use lambda_runtime::{run, service_fn};
    let func = service_fn(lambda_handler::function_handler);
    run(func).await
}
