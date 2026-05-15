// The standardoc core binary version this extension build expects.
// Bumped per binary release. Decoupled from the extension's own semver
// (the VSCode marketplace rejects `-beta.N` so ext stays on plain x.y.z)
// and from the manifest's `core_version` field — this is the source of
// truth for which release tag the installer must fetch.
export const BINARY_VERSION = '1.0.0-beta.2';
export const RELEASE_REPO = 'miralabs-tech/standardoc';
export const MANIFEST_URL = `https://github.com/${RELEASE_REPO}/releases/download/v${BINARY_VERSION}/version.json`;