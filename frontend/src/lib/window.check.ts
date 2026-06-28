import { strict as assert } from 'node:assert';
import { windowStartFor, nextWindowEnd } from './window';

// fatia: clampa em 0, nunca negativo/NaN
assert.equal(windowStartFor(5000, 120), 4880);
assert.equal(windowStartFor(80, 120), 0);
assert.equal(windowStartFor(0, 120), 0);

// encolheu (reset / /clear) -> re-ancora na cauda nova (independe de atBottom)
assert.equal(nextWindowEnd(false, 30, 5000), 30);
assert.equal(nextWindowEnd(true, 30, 5000), 30);
// colado no fim -> acompanha a cauda
assert.equal(nextWindowEnd(true, 5001, 5000), 5001);
// rolado pra cima -> congela (sem pulo)
assert.equal(nextWindowEnd(false, 5001, 5000), 5000);
// ja na cauda, colado -> no-op (garante terminacao do effect)
assert.equal(nextWindowEnd(true, 5000, 5000), 5000);

console.log('window.check OK');
