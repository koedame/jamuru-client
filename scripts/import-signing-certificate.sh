#!/usr/bin/env bash
# Puts the fixed self-signed code-signing certificate where `codesign` finds
# it as a valid identity, on a macOS runner.
#
# Tauri can import a certificate itself (APPLE_CERTIFICATE), but then looks the
# identity up with `security find-identity -v`, which skips a self-signed
# certificate nobody trusts: "failed to resolve signing identity". So the
# certificate goes into its own keychain here, and is trusted for code
# signing, before the build.
#
# Usage (APPLE_CERTIFICATE is the base64 of the .p12):
#   APPLE_CERTIFICATE=... APPLE_CERTIFICATE_PASSWORD=... \
#     scripts/import-signing-certificate.sh <identity name>
set -euo pipefail

identity=${1:?usage: import-signing-certificate.sh <identity name>}
dir=${RUNNER_TEMP:-$(mktemp -d)}
keychain="${dir}/jamuru-signing.keychain-db"
keychain_password=$(uuidgen)

printf '%s' "${APPLE_CERTIFICATE}" | base64 --decode > "${dir}/signing.p12"

security create-keychain -p "${keychain_password}" "${keychain}"
security set-keychain-settings -lut 21600 "${keychain}"
security unlock-keychain -p "${keychain_password}" "${keychain}"
security import "${dir}/signing.p12" -k "${keychain}" -P "${APPLE_CERTIFICATE_PASSWORD}" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple: -s -k "${keychain_password}" "${keychain}" > /dev/null
# shellcheck disable=SC2046
security list-keychains -d user -s "${keychain}" $(security list-keychains -d user | tr -d '"')

security find-certificate -c "${identity}" -p "${keychain}" > "${dir}/signing.pem"
sudo security add-trusted-cert -d -r trustRoot -p codeSign -k /Library/Keychains/System.keychain "${dir}/signing.pem"
rm -f "${dir}/signing.p12"

security find-identity -v -p codesigning | tee "${dir}/identities.txt"
grep -q "\"${identity}\"" "${dir}/identities.txt"
