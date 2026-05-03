#!/usr/bin/env bash
# Re-sign slack-sessions binaries with a stable self-signed certificate.
#
# Why: every `cargo install` produces a freshly ad-hoc-signed binary with a
# different code-signing identity. macOS Keychain ACL entries (and TCC
# consent records) are pinned to the signing identity, so a rebuild
# invalidates the prior "Always Allow" decisions and re-prompts the user
# 2-4 times on the next launch. Re-signing each rebuilt binary with the
# same self-signed cert keeps the identity stable, so prompts only appear
# once (the very first install).
#
# Idempotent. Safe to re-run.
set -euo pipefail

CERT_NAME="${SLACK_SESSIONS_CODESIGN_CERT:-slack-sessions-codesign}"
KEYCHAIN="${HOME}/Library/Keychains/login.keychain-db"
CARGO_BIN="${CARGO_INSTALL_ROOT:-${HOME}/.cargo}/bin"
BINARIES=("${CARGO_BIN}/slack-sessions" "${CARGO_BIN}/slack-sessionsd")

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "[--] not macOS — skipping codesign step"
    exit 0
fi

if security find-certificate -c "${CERT_NAME}" "${KEYCHAIN}" >/dev/null 2>&1; then
    echo "==> using existing code-signing cert: ${CERT_NAME}"
else
    echo "==> creating self-signed code-signing cert: ${CERT_NAME}"
    TMP="$(mktemp -d)"
    trap 'rm -rf "${TMP}"' EXIT

    cat > "${TMP}/openssl.cnf" <<EOF
[ req ]
distinguished_name = req_dn
prompt             = no
[ req_dn ]
CN = ${CERT_NAME}
[ v3 ]
keyUsage          = critical, digitalSignature
extendedKeyUsage  = critical, codeSigning
basicConstraints  = critical, CA:false
EOF

    # Use Apple's bundled LibreSSL explicitly. Homebrew's OpenSSL 3.x on
    # PATH defaults to AES-encrypted PKCS12, which macOS `security import`
    # cannot read; LibreSSL defaults to RC2-40 / 3DES which it can.
    OPENSSL=/usr/bin/openssl

    "${OPENSSL}" req -new -newkey rsa:2048 -nodes -x509 -days 3650 \
        -config "${TMP}/openssl.cnf" -extensions v3 \
        -keyout "${TMP}/key.pem" -out "${TMP}/cert.pem" >/dev/null 2>&1

    P12_PW="$("${OPENSSL}" rand -hex 16)"
    "${OPENSSL}" pkcs12 -export \
        -inkey "${TMP}/key.pem" \
        -in    "${TMP}/cert.pem" \
        -out   "${TMP}/cert.p12" \
        -name  "${CERT_NAME}" \
        -passout "pass:${P12_PW}" >/dev/null 2>&1

    # -A: allow any application to use the imported key without further
    # prompts. Acceptable here — the cert is self-signed and untrusted by
    # Gatekeeper, so its only purpose is keychain ACL stability for our
    # own binaries; it grants no real authority on the host.
    security import "${TMP}/cert.p12" \
        -k "${KEYCHAIN}" \
        -P "${P12_PW}" \
        -A >/dev/null

    echo "    [ok] cert imported into login keychain"
fi

signed=0
for BIN in "${BINARIES[@]}"; do
    if [[ -x "${BIN}" ]]; then
        codesign --force --sign "${CERT_NAME}" "${BIN}"
        echo "    [ok] signed ${BIN}"
        signed=$((signed + 1))
    else
        echo "    [--] not found, skipped: ${BIN}"
    fi
done

if [[ "${signed}" -eq 0 ]]; then
    echo "[!] no binaries signed — did cargo install run first?" >&2
    exit 1
fi
