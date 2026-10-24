/**
 * Vitest setup file
 *
 * Sets up testing utilities and global configurations for all tests.
 */

import "@testing-library/jest-dom/vitest";
import "./i18n";

import { configure } from "@testing-library/react";

// A screen that polls the backend shows what it read a few polls later. With
// every test file running at once, that can take longer than the 1 s the
// `findBy` and `waitFor` queries wait by default, and a test that would pass
// failed now and then. They wait 5 s; one that passes returns as soon as it does.
configure({ asyncUtilTimeout: 5000 });
