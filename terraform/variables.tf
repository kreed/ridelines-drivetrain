variable "aws_region" {
  description = "AWS region to deploy resources"
  type        = string
  default     = "us-west-2"
}

variable "project_name" {
  description = "Name of the project, used for resource naming"
  type        = string
  default     = "ridelines-drivetrain"
}

variable "enable_function_url" {
  description = "Enable Lambda function URL for HTTP access"
  type        = bool
  default     = false
}

variable "enable_scheduled_execution" {
  description = "Enable scheduled execution via EventBridge"
  type        = bool
  default     = false
}

variable "schedule_expression" {
  description = "EventBridge schedule expression (e.g., 'rate(1 hour)' or 'cron(0 9 * * ? *)')"
  type        = string
  default     = "rate(6 hours)"
}

variable "github_org" {
  description = "GitHub organization/username"
  type        = string
  default     = "kreed"
}

variable "github_repo" {
  description = "GitHub repository name"
  type        = string
  default     = "ridelines-drivetrain"
}
