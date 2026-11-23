// node --test test/  (no dependencies; uses fakes only, never WhatsApp)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  normalizeNumber,
  normalizeChat,
  parseAllowlist,
  isAllowed,
  inboundDecision,
  jidFor,
} from '../allowlist.js';

test('numbers normalise from every written form', () => {
  for (const raw of [
    '+41791234567',
    '+41 79 123 45 67',
    '0041 79 123 45 67',
    '41791234567',
    '+41 (79) 123-45.67',
    '41791234567@s.whatsapp.net',
    '41791234567:12@s.whatsapp.net',
    '41791234567@C.US',
    '  +41791234567  ',
  ]) {
    assert.equal(normalizeNumber(raw), '+41791234567', raw);
  }
});

test('numbers that cannot be compared safely are rejected', () => {
  for (const raw of [
    '',
    '+',
    '079 123 45 67',
    '+0791234567',
    '000791234567',
    '12345',
    '+1234567890123456',
    'abc',
    '+41 79 abc',
    '120363000000000000@g.us',
    '41791234567@example.com',
    undefined,
    null,
  ]) {
    assert.equal(normalizeNumber(raw), null, String(raw));
  }
});

test('chats normalise to number, group or privacy id', () => {
  assert.equal(normalizeChat('120363000000000000@g.us'), '120363000000000000@g.us');
  assert.equal(normalizeChat(' 41791234567-1600000000@G.US '), '41791234567-1600000000@g.us');
  assert.equal(normalizeChat('123456789012345@lid'), '123456789012345@lid');
  assert.equal(normalizeChat('123456789012345:4@lid'), '123456789012345@lid');
  assert.equal(normalizeChat('41791234567@s.whatsapp.net'), '+41791234567');
  assert.equal(normalizeChat('@g.us'), null);
  assert.equal(normalizeChat('abc@g.us'), null);
  assert.equal(normalizeChat('status@broadcast'), null);
  assert.equal(normalizeChat('*'), null);
});

test('allowlist matches every form of a listed chat', () => {
  const list = parseAllowlist('+41 79 123 45 67, +41 79 123 45 67', '120363000000000000@g.us');
  assert.equal(list.entries.size, 2);
  assert.ok(isAllowed(list, '41791234567@s.whatsapp.net'));
  assert.ok(isAllowed(list, '0041791234567'));
  assert.ok(isAllowed(list, '120363000000000000@g.us'));
  assert.ok(!isAllowed(list, '+41790000000'));
  assert.ok(!isAllowed(list, '120363999999999999@g.us'));
  assert.ok(!isAllowed(list, 'garbage'));
});

test('empty or unset allowlist allows nobody, and * is not a wildcard', () => {
  for (const list of [parseAllowlist(undefined, undefined), parseAllowlist('', ''), parseAllowlist('*', '*')]) {
    assert.ok(!isAllowed(list, '+41791234567'));
    assert.ok(!isAllowed(list, '120363000000000000@g.us'));
  }
  assert.deepEqual(parseAllowlist('*', '079 123').rejected, ['*', '079 123']);
});

test('inbound: a direct chat is decided on the sender, a group on the group', () => {
  const list = parseAllowlist('+41791234567', '120363000000000000@g.us');
  assert.ok(inboundDecision(list, { remoteJid: '41791234567@s.whatsapp.net' }).allowed);
  assert.ok(!inboundDecision(list, { remoteJid: '41790000000@s.whatsapp.net' }).allowed);

  // A group is allowed as a whole; a listed person in an unlisted group is not.
  const inGroup = inboundDecision(list, {
    remoteJid: '120363000000000000@g.us',
    participant: '41790000000@s.whatsapp.net',
  });
  assert.equal(inGroup.allowed, true);
  assert.equal(inGroup.isGroup, true);
  assert.ok(!inboundDecision(list, {
    remoteJid: '120363999999999999@g.us',
    participant: '41791234567@s.whatsapp.net',
  }).allowed);

  // A privacy id is allowed when WhatsApp also gives the listed phone number.
  assert.ok(inboundDecision(list, { remoteJid: '987654321098765@lid', senderPn: '41791234567@s.whatsapp.net' }).allowed);
  assert.ok(!inboundDecision(list, { remoteJid: '987654321098765@lid' }).allowed);

  // Nothing at all: refused, not crashed.
  assert.ok(!inboundDecision(list, undefined).allowed);
});

test('outbound: jidFor builds the JID and refuses what is not a chat', () => {
  assert.equal(jidFor('+41 79 123 45 67'), '41791234567@s.whatsapp.net');
  assert.equal(jidFor('120363000000000000@g.us'), '120363000000000000@g.us');
  assert.equal(jidFor('987654321098765@lid'), '987654321098765@lid');
  assert.equal(jidFor('079 123 45 67'), null);
  assert.equal(jidFor(''), null);
});
