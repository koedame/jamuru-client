/**
 * i18n tests based on docs-spec/behavior/i18n.feature
 *
 * i18n is implemented entirely in TypeScript (ADR-007: i18next), so these
 * scenarios are verified at the UI layer. Requirement IDs and the `Verifies:`
 * convention are defined in ADR-018.
 */

import { describe, it, expect, beforeEach, afterAll } from 'vitest';

import i18n from './index';
import { i18nConfig } from './config';
import ja from '../../locales/ja.json';
import en from '../../locales/en.json';

/** A key that exists in both bundles, used to prove the language actually switched. */
const SHARED_KEY = 'common.button.cancel';

describe('i18n', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('en');
  });

  afterAll(async () => {
    await i18n.changeLanguage('en');
  });

  describe('locale detection', () => {
    // Given the system locale is Japanese and config.toml has no language setting
    // When jamuru starts for the first time
    // Then the UI is displayed in Japanese
    //
    // The detector reads the system locale via `navigator`; what this asserts is
    // that Japanese is a detectable target and that detection consults the
    // system at all.
    //
    // Verifies: REQ-I18N-101
    it('detects the system locale and supports Japanese', () => {
      expect(i18nConfig.detection.order).toContain('navigator');
      expect(i18nConfig.supportedLngs).toContain('ja');
      expect(i18nConfig.supportedLngs).toContain('en');
      expect(i18n.options.supportedLngs).toEqual(
        expect.arrayContaining(['ja', 'en'])
      );
    });

    // Unsupported locales must land on English rather than showing raw keys.
    //
    // Verifies: REQ-I18N-101
    it('falls back to English for an unsupported locale', async () => {
      await i18n.changeLanguage('zz');
      expect(i18n.t(SHARED_KEY)).toBe(en.common.button.cancel);
    });
  });

  describe('language switching', () => {
    // Given the UI is in English
    // When the user selects Japanese
    // Then the UI switches to Japanese immediately
    //
    // Verifies: REQ-I18N-102
    it('switches the active language without a reload', async () => {
      expect(i18n.t(SHARED_KEY)).toBe(en.common.button.cancel);

      await i18n.changeLanguage('ja');

      expect(i18n.language).toBe('ja');
      expect(i18n.t(SHARED_KEY)).toBe(ja.common.button.cancel);
      expect(ja.common.button.cancel).not.toBe(en.common.button.cancel);
    });
  });

  describe('missing translations', () => {
    // Given the UI is in Japanese
    // And a key is absent from ja.json
    // When the UI renders that key
    // Then the English translation is shown
    //
    // Verifies: REQ-I18N-103
    it('falls back to English when a key is missing from the active bundle', async () => {
      const key = 'experimental.new_feature';
      const value = 'Experimental feature';

      i18n.addResource('en', 'translation', key, value);
      await i18n.changeLanguage('ja');

      // `exists` follows the fallback chain, so check the ja bundle directly.
      expect(i18n.getResource('ja', 'translation', key)).toBeUndefined();
      expect(i18n.t(key)).toBe(value);
      expect(i18nConfig.fallbackLng).toBe('en');
    });

    // A key missing from every bundle must surface as the key itself rather
    // than an empty string, so the gap is visible during development.
    it('returns the key itself when no bundle has it', () => {
      expect(i18n.t('nonexistent.key.for.testing')).toBe(
        'nonexistent.key.for.testing'
      );
    });
  });

  describe('persistence', () => {
    // Given the user changed the language to Japanese
    // When jamuru is restarted
    // Then the UI is still in Japanese
    //
    // Persistence is delegated to the detector's localStorage cache. A real
    // restart is out of scope for a unit test; what is verified here is that
    // the choice is written to the store the detector reads on next start.
    //
    // Verifies: REQ-I18N-104
    it('caches the selected language in localStorage', async () => {
      expect(i18nConfig.detection.caches).toContain('localStorage');
      expect(i18nConfig.detection.order[0]).toBe('localStorage');

      await i18n.changeLanguage('ja');

      expect(
        window.localStorage.getItem(i18nConfig.detection.lookupLocalStorage)
      ).toBe('ja');
    });
  });
});
