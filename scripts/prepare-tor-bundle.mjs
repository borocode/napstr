#!/usr/bin/env node

import { createHash } from 'node:crypto';
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const bundles = {
  'linux-x64': {
    platform: 'linux',
    url: 'https://archive.torproject.org/tor-package-archive/torbrowser/15.0.20/tor-expert-bundle-linux-x86_64-15.0.20.tar.gz',
    sha256: '3b39a2a7fbf43ef28b9ae0a6afca02a12935232f81769e4fef7472d6b5676eaf'
  },
  'win32-x64': {
    platform: 'windows',
    url: 'https://archive.torproject.org/tor-package-archive/torbrowser/15.0.20/tor-expert-bundle-windows-x86_64-15.0.20.tar.gz',
    sha256: 'd59bff934e3ad876e1623e24ae60c19aeea56f50178093b9f86fba230639f949'
  },
  'darwin-x64': {
    platform: 'macos',
    url: 'https://archive.torproject.org/tor-package-archive/torbrowser/15.0.20/tor-expert-bundle-macos-x86_64-15.0.20.tar.gz',
    sha256: '6ec3048b3a5d55e297f35d84830d0e338884d702aac3db49056633c1223841df'
  },
  'darwin-arm64': {
    platform: 'macos',
    url: 'https://archive.torproject.org/tor-package-archive/torbrowser/15.0.20/tor-expert-bundle-macos-aarch64-15.0.20.tar.gz',
    sha256: '73fdccde8136678e41a625160993e6a9dc4f4ff8cd376318b5e41e5627d55682'
  }
};

const bundle = bundles[`${process.platform}-${process.arch}`];
if (!bundle) {
  throw new Error(`No pinned Tor bundle is available for ${process.platform}-${process.arch}`);
}

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const destination = resolve(repositoryRoot, 'src-tauri', 'resources', 'tor', bundle.platform);
const archive = join(tmpdir(), `napstr-tor-${process.pid}.tar.gz`);

console.log(`Downloading pinned Tor Expert Bundle for ${process.platform}-${process.arch}…`);
const response = await fetch(bundle.url);
if (!response.ok) {
  throw new Error(`Could not download Tor Expert Bundle: HTTP ${response.status}`);
}
writeFileSync(archive, Buffer.from(await response.arrayBuffer()));

try {
  const actualSha256 = createHash('sha256').update(readFileSync(archive)).digest('hex');
  if (actualSha256 !== bundle.sha256) {
    throw new Error(`Tor Expert Bundle SHA-256 mismatch: expected ${bundle.sha256}, received ${actualSha256}`);
  }

  rmSync(destination, { recursive: true, force: true });
  mkdirSync(destination, { recursive: true });
  const extracted = spawnSync('tar', ['-xzf', archive, '-C', destination], { stdio: 'inherit' });
  if (extracted.error) throw extracted.error;
  if (extracted.status !== 0) {
    throw new Error(`Could not extract Tor Expert Bundle (tar exited with ${extracted.status})`);
  }

  for (const relativePath of ['debug', 'docs', join('tor', 'pluggable_transports')]) {
    rmSync(join(destination, relativePath), { recursive: true, force: true });
  }

  const executable = join(destination, 'tor', process.platform === 'win32' ? 'tor.exe' : 'tor');
  if (!existsSync(executable)) {
    throw new Error(`Tor Expert Bundle did not contain ${executable}`);
  }
  if (process.platform !== 'win32') chmodSync(executable, 0o755);
  console.log(`Verified bundled Tor runtime: ${executable}`);
} finally {
  rmSync(archive, { force: true });
}
