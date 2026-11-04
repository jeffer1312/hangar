import { test } from 'node:test';
import assert from 'node:assert/strict';
import { detectPlatform, deviceFor, installFor, INSTALL, nextIndex, prevIndex } from '../src/site.js';

test('celular escolhe o cartão certo', () => {
  assert.equal(deviceFor('phone', 'Mozilla/5.0 (Linux; Android 15; Pixel 9)'), 'android');
  assert.equal(deviceFor('phone', 'Mozilla/5.0 (iPhone; CPU iPhone OS 19_0 like Mac OS X)'), 'iphone');
  assert.equal(deviceFor('macos', 'Mozilla/5.0 (Macintosh)'), 'macos');
});

test('desktop por userAgent', () => {
  assert.equal(detectPlatform('Mozilla/5.0 (X11; Linux x86_64) Chrome/140', 'Linux x86_64'), 'linux');
  assert.equal(detectPlatform('Mozilla/5.0 (Windows NT 10.0; Win64; x64)', 'Win32'), 'windows');
  assert.equal(detectPlatform('Mozilla/5.0 (Macintosh; Intel Mac OS X 14_5)', 'MacIntel', 0), 'macos');
  // "Darwin" contém "win" e não pode virar Windows
  assert.equal(detectPlatform('Mozilla/5.0 (Macintosh; Intel Mac OS X 14_5) Darwin/23', 'MacIntel', 0), 'macos');
});

test('celular nunca vira Linux nem macOS', () => {
  assert.equal(detectPlatform('Mozilla/5.0 (Linux; Android 15; Pixel 9) Mobile Safari/537.36', 'Linux armv8l'), 'phone');
  assert.equal(detectPlatform('Mozilla/5.0 (iPhone; CPU iPhone OS 19_0 like Mac OS X)', 'iPhone'), 'phone');
  // iPad moderno se apresenta como Mac; o toque é o que denuncia
  assert.equal(detectPlatform('Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)', 'MacIntel', 5), 'phone');
});

test('comando de instalação por sistema', () => {
  assert.equal(installFor('windows'), 'win');
  for (const p of ['linux', 'macos', 'phone']) assert.equal(installFor(p), 'unix');
  assert.match(INSTALL.unix, /bootstrap\.sh \| bash$/);
  assert.match(INSTALL.win, /bootstrap\.ps1 \| iex$/);
});

test('abas giram nos dois sentidos', () => {
  assert.equal(nextIndex(3, 4), 0);
  assert.equal(prevIndex(0, 4), 3);
});
