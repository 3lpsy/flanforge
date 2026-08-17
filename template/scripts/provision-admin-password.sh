#!/bin/bash
# Replace the public base image's known administrator password after all other
# provisioning has completed.
set -euo pipefail

password_file=/tmp/flanforge-admin-password
cleanup_password() {
  sudo rm -f "${password_file}"
}
trap cleanup_password EXIT

case "${DISABLE_ADMIN_PASSWORD_CHANGE:-false}" in
  true)
    echo "==> administrator password change explicitly disabled"
    exit 0
    ;;
  false) ;;
  *)
    echo "DISABLE_ADMIN_PASSWORD_CHANGE must be true or false" >&2
    exit 1
    ;;
esac

sudo chmod 0600 "${password_file}"
IFS= read -r admin_password < "${password_file}" \
  || { echo "missing administrator password" >&2; exit 1; }
[ "${#admin_password}" -ge 16 ] && [ "${#admin_password}" -le 128 ] \
  || { echo "administrator password must contain 16 to 128 characters" >&2; exit 1; }
printf '%s' "${admin_password}" | LC_ALL=C grep -Eq '^[[:graph:]]{16,128}$' \
  || { echo "administrator password must contain only visible ASCII characters" >&2; exit 1; }

base_password="${BASE_ADMIN_PASSWORD:-}"
[ -n "${base_password}" ] \
  || { echo "missing base administrator password" >&2; exit 1; }

# The account holds a SecureToken, so a root record reset is refused: change the
# password as its owner, authenticated with the image's bootstrap password.
sysadminctl -oldPassword "${base_password}" -newPassword "${admin_password}"
unset base_password BASE_ADMIN_PASSWORD

# sysadminctl reports success even when the change fails, so prove it.
sudo /usr/bin/dscl . -authonly admin "${admin_password}"

# Automatic login keeps its own copy of the password, so repair it whenever the
# base image configured it. Changing the account alone would silently leave
# every clone at the login window.
login_defaults=/Library/Preferences/com.apple.loginwindow
if sudo /usr/bin/defaults read "${login_defaults}" autoLoginUser >/dev/null 2>&1; then
  sudo /usr/sbin/sysadminctl -autologin set -userName admin -password "${admin_password}"
  [ "$(sudo /usr/bin/defaults read "${login_defaults}" autoLoginUser)" = admin ] \
    || { echo "automatic login user was lost while changing the password" >&2; exit 1; }
  sudo test -s /etc/kcpassword \
    || { echo "automatic login secret was not rewritten" >&2; exit 1; }
  echo "==> automatic login preserved for admin"
fi
unset admin_password
cleanup_password
trap - EXIT
echo "==> administrator password changed"
