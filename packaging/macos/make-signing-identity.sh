#!/usr/bin/env bash
# Create a self-signed code-signing identity, "Snapoleon Local Signing", in the login keychain.
#
# Builds signed with it keep one designated requirement, so macOS keeps the Screen Recording and
# Accessibility grants across rebuilds (see bundle.sh). It is for builds made and used on this Mac only:
# other Macs do not trust it, and Gatekeeper still treats the app as unidentified when it is copied elsewhere.
# Needs an administrator password once, to trust the certificate for code signing.
set -euo pipefail

name="Snapoleon Local Signing"
if security find-identity -v -p codesigning | grep -q "\"$name\""; then
  echo "\"$name\" already exists"
  exit 0
fi

dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT
cat >"$dir/cs.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $name
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
EOF
openssl req -x509 -newkey rsa:2048 -nodes -keyout "$dir/key.pem" -out "$dir/cert.pem" -days 3650 -config "$dir/cs.cnf" 2>/dev/null
pass="$(openssl rand -hex 12)"
openssl pkcs12 -export -legacy -inkey "$dir/key.pem" -in "$dir/cert.pem" -out "$dir/id.p12" -passout "pass:$pass" 2>/dev/null \
  || openssl pkcs12 -export -inkey "$dir/key.pem" -in "$dir/cert.pem" -out "$dir/id.p12" -passout "pass:$pass"
security import "$dir/id.p12" -k "$HOME/Library/Keychains/login.keychain-db" -P "$pass" -T /usr/bin/codesign
echo "trusting the certificate for code signing (administrator password):"
sudo security add-trusted-cert -d -r trustRoot -p codeSign -k /Library/Keychains/System.keychain "$dir/cert.pem"
security find-identity -v -p codesigning | grep "\"$name\""
echo "done. If codesign asks for keychain access, choose Always Allow."
