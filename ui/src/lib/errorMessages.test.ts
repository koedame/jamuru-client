/**
 * Error message conversion tests
 *
 * Tests for converting technical error messages to user-friendly messages.
 */

import { describe, it, expect, afterAll } from "vitest";
import i18n from "../i18n";
import {
  formatClockGap,
  formatErrorForDisplay,
  getClockSkewSeconds,
  parseErrorMessage,
  getErrorCategory,
} from "./errorMessages";

describe("getErrorCategory", () => {
  describe("connection errors", () => {
    it("categorizes 'connection refused' as refused", () => {
      expect(getErrorCategory("Connection refused")).toBe("connection.refused");
    });

    it("categorizes 'connection refused' case-insensitively", () => {
      expect(getErrorCategory("CONNECTION REFUSED")).toBe("connection.refused");
    });

    it("categorizes 'timed out' as timeout", () => {
      expect(getErrorCategory("Connection timed out")).toBe("connection.timeout");
    });

    it("categorizes 'timeout' as timeout", () => {
      expect(getErrorCategory("Request timeout")).toBe("connection.timeout");
    });

    it("categorizes 'connection reset' as lost", () => {
      expect(getErrorCategory("Connection reset by peer")).toBe("connection.lost");
    });

    it("categorizes 'connection lost' as lost", () => {
      expect(getErrorCategory("Connection lost")).toBe("connection.lost");
    });

    it("categorizes 'network unreachable' as lost", () => {
      expect(getErrorCategory("Network is unreachable")).toBe("connection.lost");
    });
  });

  describe("room errors", () => {
    it("categorizes 'room not found' as notFound", () => {
      expect(getErrorCategory("Room not found")).toBe("room.notFound");
    });

    it("categorizes 'room is full' as full", () => {
      expect(getErrorCategory("Room is full")).toBe("room.full");
    });

    it("categorizes 'invalid code' as notFound", () => {
      expect(getErrorCategory("Invalid invite code")).toBe("room.notFound");
    });

    it("categorizes 'invalid password' as password", () => {
      expect(getErrorCategory("Invalid password")).toBe("room.password");
    });

    it("categorizes 'incorrect password' as password", () => {
      expect(getErrorCategory("Incorrect password")).toBe("room.password");
    });

    it("categorizes 'invalid invite link' as invite.invalidLink, not notFound", () => {
      expect(getErrorCategory("invalid invite link")).toBe("invite.invalidLink");
    });
  });

  describe("audio errors", () => {
    it("categorizes 'device not found' as device", () => {
      expect(getErrorCategory("Audio device not found")).toBe("audio.device");
    });

    it("categorizes 'permission denied' as permission", () => {
      expect(getErrorCategory("Microphone permission denied")).toBe("audio.permission");
    });

    it("categorizes 'no permission' as permission", () => {
      expect(getErrorCategory("No permission to access microphone")).toBe("audio.permission");
    });
  });

  describe("generic errors", () => {
    it("categorizes unknown errors as generic", () => {
      expect(getErrorCategory("Some unknown error happened")).toBe("generic");
    });

    it("categorizes empty string as generic", () => {
      expect(getErrorCategory("")).toBe("generic");
    });
  });
});

describe("parseErrorMessage", () => {
  it("returns parsed error with category and original message", () => {
    const result = parseErrorMessage("Connection refused by server");
    expect(result.category).toBe("connection.refused");
    expect(result.originalMessage).toBe("Connection refused by server");
  });

  it("returns i18n keys for title and message", () => {
    const result = parseErrorMessage("Connection timed out");
    expect(result.titleKey).toBe("error.connection.timeout.title");
    expect(result.messageKey).toBe("error.connection.timeout.message");
  });

  it("returns generic keys for unknown errors", () => {
    const result = parseErrorMessage("Unknown error");
    expect(result.titleKey).toBe("error.generic.title");
    expect(result.messageKey).toBe("error.generic.message");
  });
});

describe("clock skew", () => {
  const ahead = "Signaling error: This computer's clock is ahead of the server's by 375 seconds";

  it("categorizes a refusal caused by the clock as clockSkew, whichever way it is off", () => {
    expect(getErrorCategory(ahead)).toBe("connection.clockSkew");
    expect(
      getErrorCategory("This computer's clock is behind the server's by 90 seconds")
    ).toBe("connection.clockSkew");
  });

  it("reads the size of the gap out of the message", () => {
    expect(getClockSkewSeconds(ahead)).toBe(375);
    expect(getClockSkewSeconds("Connect failed: HTTP error: 401 Unauthorized")).toBeNull();
  });

  it("leaves a plain 401 as a generic error", () => {
    expect(getErrorCategory("Connect failed: HTTP error: 401 Unauthorized")).toBe("generic");
  });

  it("names the gap in minutes when it is minutes", () => {
    expect(formatClockGap(375, "en")).toBe("6 minutes");
    expect(formatClockGap(375, "ja")).toBe("6分");
  });

  it("names the gap in seconds when it is under a minute and a half, and in hours when it is hours", () => {
    expect(formatClockGap(45, "en")).toBe("45 seconds");
    expect(formatClockGap(7200, "en")).toBe("2 hours");
  });

  it("passes the size of the gap to the message so the person is told how far off the clock is", () => {
    const t = (key: string, options?: { amount: string }) =>
      options ? `${key}|${options.amount}` : key;

    expect(formatErrorForDisplay(ahead, t, "en")).toEqual({
      title: "error.connection.clockSkew.title",
      message: "error.connection.clockSkew.message|6 minutes",
    });
  });

  describe("with the shipped translations", () => {
    afterAll(async () => {
      await i18n.changeLanguage("en");
    });

    it("tells an English reader how far off the clock is", async () => {
      await i18n.changeLanguage("en");

      expect(formatErrorForDisplay(ahead, i18n.t.bind(i18n), i18n.language).message).toBe(
        "The clock is about 6 minutes off from the server's. Set the time correctly and jamjam connects again by itself."
      );
    });

    it("tells a Japanese reader how far off the clock is", async () => {
      await i18n.changeLanguage("ja");

      expect(formatErrorForDisplay(ahead, i18n.t.bind(i18n), i18n.language).message).toBe(
        "サーバーの時刻と約6分ずれています。時刻を合わせると、自動でつなぎ直します。"
      );
    });
  });
});
