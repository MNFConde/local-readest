import { describe, it, expect, afterEach } from 'vitest';
import {
  normalizeServerUrl,
  saveSelfhostedServerUrl,
  getSelfhostedServerUrl,
} from '@/services/selfhostedServerUrl';

describe('normalizeServerUrl', () => {
  it('defaults a missing scheme to http:// (browsers reject schemeless URLs)', () => {
    expect(normalizeServerUrl('127.0.0.1:8080')).toBe('http://127.0.0.1:8080');
    expect(normalizeServerUrl('mybox.lan:8080')).toBe('http://mybox.lan:8080');
  });

  it('keeps an explicit scheme and strips whitespace and trailing slashes', () => {
    expect(normalizeServerUrl('  https://sync.example.com/  ')).toBe('https://sync.example.com');
    expect(normalizeServerUrl('http://192.168.1.10:8080///')).toBe('http://192.168.1.10:8080');
  });

  it('returns empty when nothing usable remains', () => {
    expect(normalizeServerUrl('')).toBe('');
    expect(normalizeServerUrl('   ')).toBe('');
    expect(normalizeServerUrl('///')).toBe('');
  });
});

describe('saveSelfhostedServerUrl persistence', () => {
  afterEach(() => {
    localStorage.clear();
  });

  it('persists the normalized URL and clears on empty input', () => {
    saveSelfhostedServerUrl('127.0.0.1:8080');
    expect(getSelfhostedServerUrl()).toBe('http://127.0.0.1:8080');
    saveSelfhostedServerUrl('   ');
    expect(getSelfhostedServerUrl()).toBe('');
  });
});
