import * as crypto from 'node:crypto';
import * as fs from 'node:fs/promises';
import * as os from 'node:os';
import * as path from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { BINARY_VERSION, MANIFEST_URL } from './binary-version';

const execFileAsync = promisify(execFile);

export interface VersionManifest {
	readonly core_version: string;
	readonly ext_version: string;
	readonly protocol_version: number;
	readonly min_compat: { readonly core: string; readonly ext: string };
	readonly released_at: string;
	readonly binaries: Readonly<Record<string, string>>;
	readonly checksums_sha256: Readonly<Record<string, string>>;
}

export interface PlatformTarget {
	readonly triple: string;
	readonly archive: 'tar.gz' | 'zip';
	readonly exe: 'standardoc' | 'standardoc.exe';
}

export class InstallError extends Error {
	constructor(
		public readonly reason: string,
		public override readonly cause?: unknown,
	) {
		super(reason);
		this.name = 'InstallError';
	}
}

export class UnsupportedPlatformError extends InstallError {
	constructor(platform: string, arch: string) {
		super(`No standardoc binary published for platform=${platform} arch=${arch}`);
		this.name = 'UnsupportedPlatformError';
	}
}

export function currentPlatformTarget(
	platform: NodeJS.Platform = process.platform,
	arch: string = process.arch,
): PlatformTarget | null {
	if (platform === 'linux' && arch === 'x64')
		return { triple: 'x86_64-unknown-linux-gnu', archive: 'tar.gz', exe: 'standardoc' };
	if (platform === 'linux' && arch === 'arm64')
		return { triple: 'aarch64-unknown-linux-gnu', archive: 'tar.gz', exe: 'standardoc' };
	if (platform === 'darwin' && arch === 'x64')
		return { triple: 'x86_64-apple-darwin', archive: 'tar.gz', exe: 'standardoc' };
	if (platform === 'darwin' && arch === 'arm64')
		return { triple: 'aarch64-apple-darwin', archive: 'tar.gz', exe: 'standardoc' };
	if (platform === 'win32' && arch === 'x64')
		return { triple: 'x86_64-pc-windows-msvc', archive: 'zip', exe: 'standardoc.exe' };
	return null;
}

export function parseManifest(text: string): VersionManifest {
	let raw: unknown;
	try {
		raw = JSON.parse(text);
	} catch (e) {
		throw new InstallError(`manifest is not valid JSON: ${describe(e)}`);
	}
	if (!raw || typeof raw !== 'object') throw new InstallError('manifest must be a JSON object');
	const m = raw as Record<string, unknown>;
	const required = [
		'core_version',
		'ext_version',
		'protocol_version',
		'min_compat',
		'released_at',
		'binaries',
		'checksums_sha256',
	];
	for (const k of required) {
		if (!(k in m)) throw new InstallError(`manifest missing field: ${k}`);
	}
	if (typeof m.protocol_version !== 'number')
		throw new InstallError('manifest.protocol_version must be a number');
	if (!isStringRecord(m.binaries)) throw new InstallError('manifest.binaries must be string map');
	if (!isStringRecord(m.checksums_sha256))
		throw new InstallError('manifest.checksums_sha256 must be string map');
	return m as unknown as VersionManifest;
}

export function pickPlatformAsset(
	manifest: VersionManifest,
	target: PlatformTarget,
): { url: string; sha256: string } {
	const url = manifest.binaries[target.triple];
	const sha256 = manifest.checksums_sha256[target.triple];
	if (!url || !sha256) {
		throw new InstallError(`manifest has no entry for target ${target.triple}`);
	}
	return { url, sha256: sha256.toLowerCase() };
}

export function sha256Hex(buf: Buffer): string {
	return crypto.createHash('sha256').update(buf).digest('hex');
}

export function verifySha256(buf: Buffer, expected: string): void {
	const got = sha256Hex(buf);
	if (got !== expected.toLowerCase()) {
		throw new InstallError(`sha256 mismatch: expected ${expected}, got ${got}`);
	}
}

export interface InstalledBinary {
	readonly path: string;
	readonly protocol_version: number;
	readonly core_version: string;
}

export interface InstallArgs {
	readonly globalStorageDir: string;
	readonly log?: (line: string) => void;
}

export async function installBinary(args: InstallArgs): Promise<InstalledBinary> {
	const log = args.log ?? (() => {});
	const target = currentPlatformTarget();
	if (!target) throw new UnsupportedPlatformError(process.platform, process.arch);

	log(`installer: target=${target.triple} archive=${target.archive}`);
	log(`installer: fetching manifest ${MANIFEST_URL}`);
	const manifestText = await fetchText(MANIFEST_URL);
	const manifest = parseManifest(manifestText);
	log(
		`installer: manifest core=${manifest.core_version} protocol=${manifest.protocol_version} released=${manifest.released_at}`,
	);

	const { url, sha256 } = pickPlatformAsset(manifest, target);
	log(`installer: downloading ${url}`);
	const archiveBuf = await fetchBuffer(url);
	log(`installer: verifying sha256 (expected=${sha256.slice(0, 12)}…)`);
	verifySha256(archiveBuf, sha256);

	const tmpRoot = await fs.mkdtemp(path.join(os.tmpdir(), 'stdoc-install-'));
	try {
		const archivePath = path.join(tmpRoot, `archive.${target.archive}`);
		await fs.writeFile(archivePath, archiveBuf);
		log(`installer: extracting to ${tmpRoot}`);
		await extractWithTar(archivePath, tmpRoot);

		const innerDir = `standardoc-v${BINARY_VERSION}-${target.triple}`;
		const extractedBinary = path.join(tmpRoot, innerDir, target.exe);
		try {
			await fs.access(extractedBinary);
		} catch {
			throw new InstallError(
				`extracted archive did not contain expected binary at ${innerDir}/${target.exe}`,
			);
		}

		const installDir = path.join(args.globalStorageDir, 'bin', target.triple);
		const installPath = path.join(installDir, target.exe);
		await fs.mkdir(installDir, { recursive: true });
		await fs.rm(installPath, { force: true });
		await fs.copyFile(extractedBinary, installPath);
		if (process.platform !== 'win32') await fs.chmod(installPath, 0o755);

		log(`installer: installed → ${installPath}`);
		return {
			path: installPath,
			protocol_version: manifest.protocol_version,
			core_version: manifest.core_version,
		};
	} finally {
		await fs.rm(tmpRoot, { recursive: true, force: true }).catch(() => {});
	}
}

async function fetchText(url: string): Promise<string> {
	try {
		const res = await fetch(url);
		if (!res.ok) throw new InstallError(`HTTP ${res.status} fetching ${url}`);
		return await res.text();
	} catch (e) {
		if (e instanceof InstallError) throw e;
		throw new InstallError(`network failure fetching ${url}: ${describe(e)}`, e);
	}
}

async function fetchBuffer(url: string): Promise<Buffer> {
	try {
		const res = await fetch(url);
		if (!res.ok) throw new InstallError(`HTTP ${res.status} downloading ${url}`);
		const ab = await res.arrayBuffer();
		return Buffer.from(ab);
	} catch (e) {
		if (e instanceof InstallError) throw e;
		throw new InstallError(`network failure downloading ${url}: ${describe(e)}`, e);
	}
}

// `tar -xf <archive> -C <dir>` handles both .tar.gz and .zip on modern
// systems (macOS, Linux, Windows ≥10 1809 which ships bsdtar as tar.exe).
async function extractWithTar(archivePath: string, destDir: string): Promise<void> {
	try {
		await execFileAsync('tar', ['-xf', archivePath, '-C', destDir]);
	} catch (e) {
		throw new InstallError(`tar extraction failed for ${archivePath}: ${describe(e)}`, e);
	}
}

function isStringRecord(v: unknown): v is Record<string, string> {
	if (!v || typeof v !== 'object') return false;
	for (const val of Object.values(v as Record<string, unknown>)) {
		if (typeof val !== 'string') return false;
	}
	return true;
}

function describe(e: unknown): string {
	return e instanceof Error ? e.message : String(e);
}
