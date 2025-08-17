output "lambda_function_name" {
  description = "Name of the Lambda function"
  value       = aws_lambda_function.intervals_mapper.function_name
}

output "lambda_function_arn" {
  description = "ARN of the Lambda function"
  value       = aws_lambda_function.intervals_mapper.arn
}

output "s3_bucket_name" {
  description = "Name of the S3 bucket for GeoJSON storage"
  value       = aws_s3_bucket.geojson_storage.bucket
}

output "s3_bucket_arn" {
  description = "ARN of the S3 bucket for GeoJSON storage"
  value       = aws_s3_bucket.geojson_storage.arn
}

output "lambda_function_url" {
  description = "URL of the Lambda function (if enabled)"
  value       = var.enable_function_url ? aws_lambda_function_url.intervals_mapper_url[0].function_url : null
}

output "cloudwatch_log_group" {
  description = "Name of the CloudWatch log group"
  value       = aws_cloudwatch_log_group.lambda_logs.name
}

output "github_actions_role_arn" {
  description = "ARN of the IAM role for GitHub Actions"
  value       = aws_iam_role.github_actions.arn
}

output "github_actions_role_name" {
  description = "Name of the IAM role for GitHub Actions"
  value       = aws_iam_role.github_actions.name
}