#!/usr/bin/env node

import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
const checkOnly = process.argv.includes('--check');
const requestedTag = process.argv.slice(2).find((argument) => argument !== '--check') ?? process.env.RELEASE_TAG ?? '';
const match = /^v(\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?)$/.exec(requestedTag);

if (!match) {
  throw new Error(
    `Release tag must be valid SemVer such as v0.2.3 or v0.2.3-beta.1; received ${JSON.stringify(requestedTag)}`
  );
}

const version = match[1];
const paths = {
  packageJson: resolve(repositoryRoot, 'package.json'),
  packageLock: resolve(repositoryRoot, 'package-lock.json'),
  tauriConfig: resolve(repositoryRoot, 'src-tauri/tauri.conf.json'),
  cargoToml: resolve(repositoryRoot, 'src-tauri/Cargo.toml'),
  cargoLock: resolve(repositoryRoot, 'src-tauri/Cargo.lock')
};

function updateJson(path, update) {
  const document = JSON.parse(readFileSync(path, 'utf8'));
  update(document);
  writeFileSync(path, `${JSON.stringify(document, null, 2)}\n`);
}

function replaceExactlyOnce(path, pattern, replacement, label) {
  const original = readFileSync(path, 'utf8');
  const matches = original.match(new RegExp(pattern.source, pattern.flags.includes('g') ? pattern.flags : `${pattern.flags}g`));
  if (matches?.length !== 1) {
    throw new Error(`Expected exactly one ${label} in ${path}, found ${matches?.length ?? 0}`);
  }
  writeFileSync(path, original.replace(pattern, replacement));
}

if (!checkOnly) {
  updateJson(paths.packageJson, (document) => {
    document.version = version;
  });
  updateJson(paths.packageLock, (document) => {
    document.version = version;
    document.packages[''].version = version;
  });
  updateJson(paths.tauriConfig, (document) => {
    document.version = version;
  });
  replaceExactlyOnce(
    paths.cargoToml,
    /(\[package\][\s\S]*?\nversion\s*=\s*")[^"]+("\s*\n)/,
    `$1${version}$2`,
    'Cargo package version'
  );
  replaceExactlyOnce(
    paths.cargoLock,
    /(\[\[package\]\]\nname = "napstr"\nversion = ")[^"]+("\n)/,
    `$1${version}$2`,
    'Napstr Cargo lock entry'
  );
}

const packageJson = JSON.parse(readFileSync(paths.packageJson, 'utf8'));
const packageLock = JSON.parse(readFileSync(paths.packageLock, 'utf8'));
const tauriConfig = JSON.parse(readFileSync(paths.tauriConfig, 'utf8'));
const cargoToml = readFileSync(paths.cargoToml, 'utf8');
const cargoLock = readFileSync(paths.cargoLock, 'utf8');
const actualVersions = {
  'package.json': packageJson.version,
  'package-lock.json': packageLock.version,
  'package-lock.json root package': packageLock.packages?.['']?.version,
  'tauri.conf.json': tauriConfig.version,
  'Cargo.toml': /\[package\][\s\S]*?\nversion\s*=\s*"([^"]+)"/.exec(cargoToml)?.[1],
  'Cargo.lock': /\[\[package\]\]\nname = "napstr"\nversion = "([^"]+)"/.exec(cargoLock)?.[1]
};

for (const [source, actualVersion] of Object.entries(actualVersions)) {
  if (actualVersion !== version) {
    throw new Error(`${source} has version ${actualVersion ?? '(missing)'}, expected ${version}`);
  }
}

console.log(`Napstr release version verified: ${version}`);
