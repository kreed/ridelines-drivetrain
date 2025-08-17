terraform {
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }

  backend "s3" {
    bucket = "tofu-042666117628"
    key    = "ridelines-drivetrain/terraform.tfstate"
  }
}

provider "aws" {
  region = var.aws_region
}

# Data source to get the tippecanoe layer ARN from SSM
data "aws_ssm_parameter" "tippecanoe_layer_arn" {
  name = "/ridelines-drivetrain/tippecanoe-layer-arn"
}

# IAM role for Lambda
resource "aws_iam_role" "lambda_role" {
  name = "${var.project_name}-lambda-role"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Action = "sts:AssumeRole"
        Effect = "Allow"
        Principal = {
          Service = "lambda.amazonaws.com"
        }
      }
    ]
  })
}

# Attach basic Lambda execution policy
resource "aws_iam_role_policy_attachment" "lambda_basic_execution" {
  role       = aws_iam_role.lambda_role.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole"
}

# CloudWatch log group
resource "aws_cloudwatch_log_group" "lambda_logs" {
  name              = "/aws/lambda/${var.project_name}"
  retention_in_days = 14
}

# Data source for GitHub's OIDC provider
data "aws_iam_openid_connect_provider" "github" {
  url = "https://token.actions.githubusercontent.com"
}

# IAM role for GitHub Actions
resource "aws_iam_role" "github_actions" {
  name = "${var.project_name}-github-actions"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Principal = {
          Federated = data.aws_iam_openid_connect_provider.github.arn
        }
        Action = "sts:AssumeRoleWithWebIdentity"
        Condition = {
          StringEquals = {
            "token.actions.githubusercontent.com:aud" = "sts.amazonaws.com"
          }
          StringLike = {
            "token.actions.githubusercontent.com:sub" = [
              "repo:${var.github_org}/${var.github_repo}:*"
            ]
          }
        }
      }
    ]
  })

  tags = {
    Project   = var.project_name
    ManagedBy = "terraform"
  }
}

# IAM policy for Lambda deployment
resource "aws_iam_policy" "lambda_deployment" {
  name        = "${var.project_name}-lambda-deployment"
  description = "Policy for GitHub Actions to deploy Lambda function"

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "lambda:UpdateFunctionCode",
          "lambda:UpdateFunctionConfiguration",
          "lambda:GetFunction",
          "lambda:CreateFunction",
          "lambda:DeleteFunction",
          "lambda:AddPermission",
          "lambda:RemovePermission",
          "lambda:InvokeFunction"
        ]
        Resource = "arn:aws:lambda:${var.aws_region}:*:function:${var.project_name}"
      },
      {
        Effect = "Allow"
        Action = [
          "lambda:ListLayers",
          "lambda:GetLayerVersion"
        ]
        Resource = "*"
      }
    ]
  })

  tags = {
    Project   = var.project_name
    ManagedBy = "terraform"
  }
}

# IAM policy for SSM parameter access
resource "aws_iam_policy" "ssm_parameter_access" {
  name        = "${var.project_name}-ssm-parameter-access"
  description = "Policy for GitHub Actions to access SSM parameters"

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "ssm:GetParameter",
          "ssm:PutParameter",
          "ssm:GetParameters"
        ]
        Resource = "arn:aws:ssm:${var.aws_region}:*:parameter/ridelines-drivetrain/*"
      }
    ]
  })

  tags = {
    Project   = var.project_name
    ManagedBy = "terraform"
  }
}

# Attach policies to the GitHub Actions role
resource "aws_iam_role_policy_attachment" "lambda_deployment" {
  role       = aws_iam_role.github_actions.name
  policy_arn = aws_iam_policy.lambda_deployment.arn
}

resource "aws_iam_role_policy_attachment" "ssm_parameter_access" {
  role       = aws_iam_role.github_actions.name
  policy_arn = aws_iam_policy.ssm_parameter_access.arn
}

# S3 bucket for storing GeoJSON files
resource "aws_s3_bucket" "geojson_storage" {
  bucket = "${var.project_name}-geojson-${random_id.bucket_suffix.hex}"
}

resource "random_id" "bucket_suffix" {
  byte_length = 4
}

resource "aws_s3_bucket_versioning" "geojson_storage_versioning" {
  bucket = aws_s3_bucket.geojson_storage.id
  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "geojson_storage_encryption" {
  bucket = aws_s3_bucket.geojson_storage.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

# AWS Secrets Manager secret for Intervals API key
resource "aws_secretsmanager_secret" "intervals_api_key" {
  name        = "${var.project_name}-intervals-api-key"
  description = "API key for intervals.icu"
}

# IAM policy for S3 access
resource "aws_iam_role_policy" "lambda_s3_policy" {
  name = "${var.project_name}-s3-policy"
  role = aws_iam_role.lambda_role.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "s3:GetObject",
          "s3:PutObject",
          "s3:DeleteObject",
          "s3:ListBucket"
        ]
        Resource = [
          aws_s3_bucket.geojson_storage.arn,
          "${aws_s3_bucket.geojson_storage.arn}/*"
        ]
      },
      {
        Effect = "Allow"
        Action = [
          "s3:PutObject",
          "s3:PutObjectAcl"
        ]
        Resource = [
          "arn:aws:s3:::kreed.org-website/*"
        ]
      }
    ]
  })
}

# IAM policy for Secrets Manager access
resource "aws_iam_role_policy" "lambda_secrets_policy" {
  name = "${var.project_name}-secrets-policy"
  role = aws_iam_role.lambda_role.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "secretsmanager:GetSecretValue"
        ]
        Resource = [
          aws_secretsmanager_secret.intervals_api_key.arn
        ]
      }
    ]
  })
}

# Lambda function
resource "aws_lambda_function" "ridelines_drivetrain" {
  filename         = "../target/lambda/${var.project_name}/bootstrap.zip"
  function_name    = var.project_name
  role             = aws_iam_role.lambda_role.arn
  handler          = "bootstrap"
  runtime          = "provided.al2023"
  timeout          = 600
  memory_size      = 2048
  source_code_hash = filebase64sha256("../target/lambda/${var.project_name}/bootstrap.zip")

  layers = [
    data.aws_ssm_parameter.tippecanoe_layer_arn.value
  ]

  environment {
    variables = {
      SECRETS_MANAGER_SECRET_ARN = aws_secretsmanager_secret.intervals_api_key.arn
      S3_BUCKET                  = aws_s3_bucket.geojson_storage.bucket
      RUST_LOG                   = "info"
    }
  }

  depends_on = [
    aws_iam_role_policy_attachment.lambda_basic_execution,
    aws_cloudwatch_log_group.lambda_logs,
  ]
}

# Lambda function URL (optional - for HTTP access)
resource "aws_lambda_function_url" "ridelines_drivetrain_url" {
  count              = var.enable_function_url ? 1 : 0
  function_name      = aws_lambda_function.ridelines_drivetrain.function_name
  authorization_type = "NONE"
}

# EventBridge rule for scheduled execution (optional)
resource "aws_cloudwatch_event_rule" "schedule" {
  count               = var.enable_scheduled_execution ? 1 : 0
  name                = "${var.project_name}-schedule"
  description         = "Trigger ridelines-drivetrain Lambda function"
  schedule_expression = var.schedule_expression
}

resource "aws_cloudwatch_event_target" "lambda_target" {
  count     = var.enable_scheduled_execution ? 1 : 0
  rule      = aws_cloudwatch_event_rule.schedule[0].name
  target_id = "IntervalsMapperLambda"
  arn       = aws_lambda_function.ridelines_drivetrain.arn
}

resource "aws_lambda_permission" "allow_eventbridge" {
  count         = var.enable_scheduled_execution ? 1 : 0
  statement_id  = "AllowExecutionFromEventBridge"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.ridelines_drivetrain.function_name
  principal     = "events.amazonaws.com"
  source_arn    = aws_cloudwatch_event_rule.schedule[0].arn
}
