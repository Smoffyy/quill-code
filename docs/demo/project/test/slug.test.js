import { test } from 'node:test';
import assert from 'node:assert/strict';
import { slugify } from '../src/slug.js';

test('joins words with single dashes', () => {
  assert.equal(slugify('Hello   World'), 'hello-world');
});

test('drops punctuation and trims the ends', () => {
  assert.equal(slugify('  Ship it, now!  '), 'ship-it-now');
});