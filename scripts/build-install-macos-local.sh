#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
signing_dir="${HOME}/Library/Application Support/moe.mutsumi.mail/local-signing"
rcodesign_version="0.29.0"
rcodesign_path="${signing_dir}/bin/rcodesign"
certificate_base="${signing_dir}/mutsumi-mail-local"
certificate_path="${certificate_base}.crt"
private_key_path="${certificate_base}.key"
built_app="${repo_root}/src-tauri/target/release/bundle/macos/Mutsumi Mail.app"
installed_app="/Applications/Mutsumi Mail.app"
install_staging="/Applications/.Mutsumi Mail.installing.$$"
install_backup="/Applications/.Mutsumi Mail.previous.$$"
temporary_dir=""
installation_started=false

cleanup() {
  local exit_status=$?
  trap - EXIT

  if [[ -n "${temporary_dir}" && -d "${temporary_dir}" ]]; then
    rm -rf -- "${temporary_dir}"
  fi
  rm -rf -- "${repo_root}/dist" "${repo_root}/src-tauri/target" "${repo_root}/node_modules/.vite"
  find "${repo_root}" -maxdepth 1 -type f -name '*.tsbuildinfo' -delete

  if [[ ${exit_status} -ne 0 ]]; then
    rm -rf -- "${install_staging}"
    if [[ "${installation_started}" == true ]]; then
      rm -rf -- "${installed_app}"
      if [[ -d "${install_backup}" ]]; then
        mv -- "${install_backup}" "${installed_app}"
      fi
    fi
  else
    rm -rf -- "${install_staging}" "${install_backup}"
  fi

  exit "${exit_status}"
}
trap cleanup EXIT

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "This installer only supports macOS." >&2
  exit 1
fi

ensure_temporary_dir() {
  if [[ -z "${temporary_dir}" ]]; then
    temporary_dir="$(mktemp -d "${TMPDIR:-/tmp}/mutsumi-mail-signing.XXXXXX")"
  fi
}

case "$(uname -m)" in
  arm64)
    rcodesign_archive="apple-codesign-${rcodesign_version}-aarch64-apple-darwin.tar.gz"
    rcodesign_archive_sha256="d1a532150adaf90048260d76359261aa716abafc45c53c5dc18845029184334a"
    rcodesign_binary_sha256="6c4623db45f1d89af439a2ce42fd65798ef56aaaa3e4ced48879be05f750aacb"
    ;;
  x86_64)
    rcodesign_archive="apple-codesign-${rcodesign_version}-x86_64-apple-darwin.tar.gz"
    rcodesign_archive_sha256="14ef11bedd51a8d95eafd767939ae96d5900e5a61511bef75bb21db6e7c74140"
    rcodesign_binary_sha256="21588902f0698182c21b14d9623c424eb32595f675ace5b31dc1c3f3b0223ec1"
    ;;
  *)
    echo "Unsupported macOS architecture: $(uname -m)" >&2
    exit 1
    ;;
esac

mkdir -p -- "${signing_dir}/bin"
chmod 700 "${signing_dir}" "${signing_dir}/bin"

rcodesign_valid=false
if [[ -x "${rcodesign_path}" ]]; then
  installed_sha256="$(shasum -a 256 "${rcodesign_path}" | awk '{print $1}')"
  if [[ "${installed_sha256}" == "${rcodesign_binary_sha256}" ]]; then
    rcodesign_valid=true
  fi
fi

if [[ "${rcodesign_valid}" != true ]]; then
  ensure_temporary_dir
  archive_path="${temporary_dir}/${rcodesign_archive}"
  download_url="https://github.com/indygreg/apple-platform-rs/releases/download/apple-codesign%2F${rcodesign_version}/${rcodesign_archive}"
  curl --fail --location --proto '=https' --tlsv1.2 --output "${archive_path}" "${download_url}"
  printf '%s  %s\n' "${rcodesign_archive_sha256}" "${archive_path}" | shasum -a 256 -c -
  tar -xzf "${archive_path}" -C "${temporary_dir}"
  extracted_rcodesign="${temporary_dir}/${rcodesign_archive%.tar.gz}/rcodesign"
  extracted_sha256="$(shasum -a 256 "${extracted_rcodesign}" | awk '{print $1}')"
  if [[ "${extracted_sha256}" != "${rcodesign_binary_sha256}" ]]; then
    echo "Extracted rcodesign binary failed SHA-256 verification." >&2
    exit 1
  fi
  install -m 755 "${extracted_rcodesign}" "${rcodesign_path}"
fi

if [[ ! -s "${certificate_path}" || ! -s "${private_key_path}" ]]; then
  ensure_temporary_dir
  generated_base="${temporary_dir}/mutsumi-mail-local"
  "${rcodesign_path}" generate-self-signed-certificate \
    --profile apple-development \
    --team-id MUTSUMILCL \
    --person-name "Mutsumi Mail Local Development" \
    --validity-days 3650 \
    --pem-filename "${generated_base}"
  install -m 644 "${generated_base}.crt" "${certificate_path}"
  install -m 600 "${generated_base}.key" "${private_key_path}"
fi
chmod 644 "${certificate_path}"
chmod 600 "${private_key_path}"

cd -- "${repo_root}"
CARGO_INCREMENTAL=0 pnpm tauri build --bundles app

if [[ ! -d "${built_app}" ]]; then
  echo "Tauri build did not produce ${built_app}" >&2
  exit 1
fi

"${rcodesign_path}" sign \
  --pem-file "${certificate_path}" \
  --pem-file "${private_key_path}" \
  --timestamp-url none \
  "${built_app}"

codesign --verify --deep --strict --verbose=2 "${built_app}"
designated_requirement="$(codesign -d -r- "${built_app}" 2>&1)"
if [[ "${designated_requirement}" != *'identifier "moe.mutsumi.mail"'* || "${designated_requirement}" != *'certificate root ='* ]]; then
  echo "Refusing to install an app without the stable certificate-backed requirement." >&2
  echo "${designated_requirement}" >&2
  exit 1
fi

if pgrep -x mutsumi-mail >/dev/null; then
  osascript -e 'tell application id "moe.mutsumi.mail" to quit' || true
  for _attempt in {1..20}; do
    if ! pgrep -x mutsumi-mail >/dev/null; then
      break
    fi
    sleep 0.25
  done
  if pgrep -x mutsumi-mail >/dev/null; then
    echo "Mutsumi Mail is still running; quit it before installation." >&2
    exit 1
  fi
fi

rm -rf -- "${install_staging}" "${install_backup}"
ditto "${built_app}" "${install_staging}"
codesign --verify --deep --strict --verbose=2 "${install_staging}"

if [[ -e "${installed_app}" ]]; then
  mv -- "${installed_app}" "${install_backup}"
fi
installation_started=true
mv -- "${install_staging}" "${installed_app}"
codesign --verify --deep --strict --verbose=2 "${installed_app}"

echo "Installed ${installed_app}"
echo "${designated_requirement}"
