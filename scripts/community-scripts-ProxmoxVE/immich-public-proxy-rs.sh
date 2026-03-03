#!/usr/bin/env bash

# Copyright (c) 2021-2026 community-scripts ORG
# Author: vhsdream, daniel5151
# License: MIT | https://github.com/community-scripts/ProxmoxVE/raw/main/LICENSE
# Source: https://github.com/daniel5151/immich-public-proxy-rs

if ! command -v curl &>/dev/null; then
  printf "\r\e[2K%b" '\033[93m Setup Source \033[m' >&2
  apt-get update >/dev/null 2>&1
  apt-get install -y curl >/dev/null 2>&1
fi
source <(curl -fsSL https://raw.githubusercontent.com/community-scripts/ProxmoxVE/main/misc/core.func)
source <(curl -fsSL https://raw.githubusercontent.com/community-scripts/ProxmoxVE/main/misc/tools.func)
source <(curl -fsSL https://raw.githubusercontent.com/community-scripts/ProxmoxVE/main/misc/error_handler.func)
source <(curl -fsSL https://raw.githubusercontent.com/community-scripts/ProxmoxVE/main/misc/api.func) 2>/dev/null || true

# Enable error handling
set -Eeuo pipefail
trap 'error_handler' ERR

# ==============================================================================
# CONFIGURATION
# ==============================================================================
APP="Immich Public Proxy"
APP_TYPE="addon"
INSTALL_PATH="/opt/immich-proxy-rs"
CONFIG_PATH="/opt/immich-proxy-rs/app"
DEFAULT_PORT=3000

# Initialize all core functions (colors, formatting, icons, $STD mode)
load_functions
init_tool_telemetry "" "addon"

# ==============================================================================
# HEADER
# ==============================================================================
function header_info {
  clear
  cat <<"EOF"
    ____                    _      __          ____
   /  _/___ ___  ____ ___  (_)____/ /_        / __ \_________  _  ____  __
   / // __ `__ \/ __ `__ \/ / ___/ __ \______/ /_/ / ___/ __ \| |/_/ / / /
 _/ // / / / / / / / / / / / /__/ / / /_____/ ____/ /  / /_/ />  </ /_/ /
/___/_/ /_/ /_/_/ /_/ /_/_/\___/_/ /_/     /_/   /_/   \____/_/|_|\__, /
                                                                 /____/
EOF
}

# ==============================================================================
# OS DETECTION
# ==============================================================================
if [[ -f "/etc/alpine-release" ]]; then
  msg_error "Alpine is not supported for ${APP}. Use Debian."
  exit 238
elif [[ -f "/etc/debian_version" ]]; then
  OS="Debian"
  SERVICE_PATH="/etc/systemd/system/immich-proxy-rs.service"
else
  echo -e "${CROSS} Unsupported OS detected. Exiting."
  exit 238
fi

# ==============================================================================
# UNINSTALL
# ==============================================================================
function uninstall() {
  msg_info "Uninstalling ${APP}"
  systemctl disable --now immich-proxy-rs.service &>/dev/null || true
  rm -f "$SERVICE_PATH"
  rm -rf "$INSTALL_PATH"
  rm -f "/usr/local/bin/update_immich-public-proxy-rs"
  msg_ok "${APP} has been uninstalled"
}

function get_source_dir() {
  while true; do
    read -rp "${TAB3}Enter the path to the directory containing immich-public-proxy-rs and site/: " SOURCE_DIR
    if [[ -d "$SOURCE_DIR" && -f "$SOURCE_DIR/immich-public-proxy-rs" && -d "$SOURCE_DIR/site" ]]; then
      break
    else
      msg_warn "Directory must contain immich-public-proxy-rs binary and site/ directory! Please try again."
    fi
  done
}

# ==============================================================================
# UPDATE
# ==============================================================================
function update() {
  get_source_dir

  msg_info "Stopping service"
  systemctl stop immich-proxy-rs.service &>/dev/null || true
  msg_ok "Stopped service"

  msg_info "Backing up configuration"
  cp "$CONFIG_PATH"/.env /tmp/ipprs.env.bak 2>/dev/null || true
  msg_ok "Backed up configuration"

  msg_info "Installing ${APP}"
  mkdir -p "$CONFIG_PATH"
  cp "$SOURCE_DIR/immich-public-proxy-rs" "$CONFIG_PATH/"
  rm -rf "$CONFIG_PATH/site"
  cp -r "$SOURCE_DIR/site" "$CONFIG_PATH/"
  chmod +x "$CONFIG_PATH/immich-public-proxy-rs"
  cp "$(realpath "$0")" "$INSTALL_PATH/immich-public-proxy-rs.sh"
  msg_ok "Installed ${APP}"

  msg_info "Restoring configuration"
  cp /tmp/ipprs.env.bak "$CONFIG_PATH"/.env 2>/dev/null || true
  rm -f /tmp/ipprs.env.bak
  msg_ok "Restored configuration"

  msg_info "Updating service"
  create_service
  msg_ok "Updated service"

  msg_info "Starting service"
  systemctl start immich-proxy
  msg_ok "Started service"
  msg_ok "Updated successfully"
  exit
}

function create_service() {
  cat <<EOF >"$SERVICE_PATH"
[Unit]
Description=Immich Public Proxy
After=network.target

[Service]
Type=simple
User=root
WorkingDirectory=${INSTALL_PATH}/app
EnvironmentFile=${CONFIG_PATH}/.env
ExecStart=${INSTALL_PATH}/app/immich-public-proxy-rs
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
EOF
  systemctl daemon-reload
}

# ==============================================================================
# INSTALL
# ==============================================================================
function install() {
  get_source_dir

  msg_info "Installing ${APP}"
  mkdir -p "$CONFIG_PATH"
  cp "$SOURCE_DIR/immich-public-proxy-rs" "$CONFIG_PATH/"
  cp -r "$SOURCE_DIR/site" "$CONFIG_PATH/"
  chmod +x "$CONFIG_PATH/immich-public-proxy-rs"
  cp "$(realpath "$0")" "$INSTALL_PATH/immich-public-proxy-rs.sh"
  msg_ok "Installed ${APP}"

  MAX_ATTEMPTS=3
  attempt=0
  while true; do
    attempt=$((attempt + 1))
    read -rp "${TAB3}Enter your LOCAL Immich IP or domain (ex. 192.168.1.100 or immich.local.lan): " DOMAIN
    if [[ -z "$DOMAIN" ]]; then
      if ((attempt >= MAX_ATTEMPTS)); then
        DOMAIN="${LOCAL_IP:-localhost}"
        msg_warn "Using fallback: $DOMAIN"
        break
      fi
      msg_warn "Domain cannot be empty! (Attempt $attempt/$MAX_ATTEMPTS)"
    elif [[ "$DOMAIN" =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}$ ]]; then
      valid_ip=true
      IFS='.' read -ra octets <<<"$DOMAIN"
      for octet in "${octets[@]}"; do
        if ((octet > 255)); then
          valid_ip=false
          break
        fi
      done
      if $valid_ip; then
        break
      else
        msg_warn "Invalid IP address!"
      fi
    elif [[ "$DOMAIN" =~ ^[a-zA-Z0-9]([a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(\.[a-zA-Z0-9]([a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*\.[a-zA-Z]{2,}$ || "$DOMAIN" == "localhost" ]]; then
      break
    else
      msg_warn "Invalid domain format!"
    fi
  done

  msg_info "Creating configuration"
  cat <<EOF >"$CONFIG_PATH"/.env
IMMICH_URL=http://${DOMAIN}:2283
LEPTOS_OUTPUT_NAME=immich-public-proxy-rs
LEPTOS_SITE_ROOT=site
LEPTOS_SITE_ADDR=0.0.0.0:${DEFAULT_PORT}
RUST_LOG=info
EOF
  chmod 600 "$CONFIG_PATH"/.env
  msg_ok "Created configuration"

  msg_info "Creating service"
  create_service
  systemctl enable -q --now immich-proxy
  msg_ok "Created and started service"

  # Create update script (simple wrapper that calls the local script with type=update)
  msg_info "Creating update script"
  cat <<UPDATEEOF >/usr/local/bin/update_immich-public-proxy-rs
#!/usr/bin/env bash
# Immich Public Proxy Update Script
type=update bash "$INSTALL_PATH/immich-public-proxy-rs.sh"
UPDATEEOF
  chmod +x /usr/local/bin/update_immich-public-proxy-rs
  msg_ok "Created update script (/usr/local/bin/update_immich-public-proxy-rs)"

  echo ""
  msg_ok "${APP} is reachable at: ${BL}http://${LOCAL_IP}:${DEFAULT_PORT}${CL}"
  echo ""
}

# ==============================================================================
# MAIN
# ==============================================================================

# Handle type=update (called from update script)
if [[ "${type:-}" == "update" ]]; then
  header_info
  if [[ -d "$INSTALL_PATH" && -f "$SERVICE_PATH" ]]; then
    update
  else
    msg_error "${APP} is not installed. Nothing to update."
    exit 233
  fi
  exit 0
fi

header_info
get_lxc_ip

# Check if already installed
if [[ -d "$INSTALL_PATH" && -f "$SERVICE_PATH" ]]; then
  msg_warn "${APP} is already installed."
  echo ""

  echo -n "${TAB}Uninstall ${APP}? (y/N): "
  read -r uninstall_prompt
  if [[ "${uninstall_prompt,,}" =~ ^(y|yes)$ ]]; then
    uninstall
    exit 0
  fi

  echo -n "${TAB}Update ${APP}? (y/N): "
  read -r update_prompt
  if [[ "${update_prompt,,}" =~ ^(y|yes)$ ]]; then
    update
    exit 0
  fi

  msg_warn "No action selected. Exiting."
  exit 0
fi

# Fresh installation
msg_warn "${APP} is not installed."
echo ""
echo -e "${TAB}${INFO} This will install:"
echo -e "${TAB}  - Immich Public Proxy (Rust Version)"
echo ""

echo -n "${TAB}Install ${APP}? (y/N): "
read -r install_prompt
if [[ "${install_prompt,,}" =~ ^(y|yes)$ ]]; then
  install
else
  msg_warn "Installation cancelled. Exiting."
  exit 0
fi
