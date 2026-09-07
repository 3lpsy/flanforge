#!/usr/bin/env bash
# Load trusted template-local settings without coupling unrelated commands.

source_template_env_file() {
  local env_file="$1"
  local allexport_was_enabled=false

  [ -f "${env_file}" ] || {
    echo "error: template environment is not a regular file: ${env_file}" >&2
    return 1
  }
  case "$-" in
    *a*) allexport_was_enabled=true ;;
  esac

  echo "==> loading template settings from ${env_file}"
  set -a
  # shellcheck disable=SC1090
  source "${env_file}"
  if [ "${allexport_was_enabled}" = true ]; then
    set -a
  else
    set +a
  fi
}

load_template_environment() {
  local template_dir="$1"
  local env_file="${template_dir}/.env"

  if [ -e "${env_file}" ] || [ -L "${env_file}" ]; then
    source_template_env_file "${env_file}"
  fi
}

load_tart_template_environment() {
  local template_dir="$1"
  local project_root="$2"
  local template_env="${template_dir}/.env"
  local legacy_env="${project_root}/.env"

  if [ -e "${template_env}" ] || [ -L "${template_env}" ]; then
    source_template_env_file "${template_env}"
    return
  fi
  if [ -e "${legacy_env}" ] || [ -L "${legacy_env}" ]; then
    echo "warning: root .env fallback is deprecated; copy settings to templates/tart/.env" >&2
    source_template_env_file "${legacy_env}"
  fi
}
