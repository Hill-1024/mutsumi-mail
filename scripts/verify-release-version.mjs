import { readFile } from "node:fs/promises";

const tag = process.argv.slice(2).find((argument) => argument !== "--") ?? process.env.GITHUB_REF_NAME;

if (!tag || !/^v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(tag)) {
  throw new Error("Release tags must use the form v<version>, for example v0.1.0.");
}

const expectedVersion = tag.slice(1);
const [packageJson, tauriConfig, cargoToml] = await Promise.all([
  readFile(new URL("../package.json", import.meta.url), "utf8").then(JSON.parse),
  readFile(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8").then(JSON.parse),
  readFile(new URL("../src-tauri/Cargo.toml", import.meta.url), "utf8"),
]);
const cargoVersion = cargoToml.match(/^version\s*=\s*"([^"]+)"\s*$/m)?.[1];
const versions = {
  "package.json": packageJson.version,
  "src-tauri/tauri.conf.json": tauriConfig.version,
  "src-tauri/Cargo.toml": cargoVersion,
};
const mismatches = Object.entries(versions).filter(([, version]) => version !== expectedVersion);

if (mismatches.length > 0) {
  const details = Object.entries(versions)
    .map(([file, version]) => `${file}: ${version ?? "missing"}`)
    .join("\n");
  throw new Error(`Tag ${tag} does not match every application version.\n${details}`);
}

process.stdout.write(`Release tag ${tag} matches all application version files.\n`);
