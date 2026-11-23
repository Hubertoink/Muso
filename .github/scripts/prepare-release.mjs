import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { basename, join } from 'node:path';

const { version } = JSON.parse(readFileSync('src-tauri/tauri.conf.json', 'utf8'));
const tag = process.env.GITHUB_REF_NAME;
if (tag !== `v${version}`) throw new Error('Release tag must match the app version.');

const bundles = {
  'windows-x86_64-nsis': `Muso_${version}_x64-setup.exe`,
  'linux-x86_64-appimage': `Muso_${version}_amd64.AppImage`,
  'linux-x86_64-deb': `Muso_${version}_amd64.deb`,
};
const files = readdirSync('artifacts', { recursive: true });
const platforms = {};
mkdirSync('release');
for (const [platform, name] of Object.entries(bundles)) {
  const matches = files.filter(file => basename(file) === name);
  if (matches.length !== 1) throw new Error(`Expected one ${name}, found ${matches.length}`);
  const target = join('release', name);
  copyFileSync(join('artifacts', matches[0]), target);
  execFileSync(process.execPath, ['node_modules/@tauri-apps/cli/tauri.js', 'signer', 'sign', '--app-version', version, target], { stdio: 'inherit' });
  platforms[platform] = {
    url: `https://github.com/${process.env.GITHUB_REPOSITORY}/releases/download/${tag}/${name}`,
    signature: readFileSync(target + '.sig', 'utf8').trim(),
  };
}
writeFileSync('release/latest.json', JSON.stringify({version, platforms}, null, 2) + '\n');
