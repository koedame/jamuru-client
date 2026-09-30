/**
 * ConnectionPanel - Session connection interface
 * jamjam brand guide compliant (ui.pen Screens/JoinRoom), no Tauri dependencies
 */
import { useState, useCallback, type FormEvent, type KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import { INVITE_CODE_LENGTH, isValidInviteCode } from "../../lib/inviteCode";
import "./ConnectionPanel.css";

export type ConnectionState = "idle" | "connecting" | "error";

/**
 * What an "error" state is about: a room-level failure (invalid code, room
 * full - the user is still connected to the signaling server and can retype
 * a code) versus a failure to reach the signaling server itself (no
 * connection exists yet at all).
 */
export type ConnectionErrorKind = "room" | "server";

export interface ConnectionPanelProps {
  /** Current state */
  state?: ConnectionState;
  /** Input code value */
  code?: string;
  /** Error message to display */
  errorMessage?: string;
  /** What the "error" state is about (default "room") */
  errorKind?: ConnectionErrorKind;
  /** Raw, untranslated error text from the backend, shown alongside
   * `errorMessage` when `errorKind` is "server" so the original diagnostic
   * is not lost behind the friendly summary. */
  rawErrorMessage?: string;
  /** The signaling server this attempt is/was dialing, shown while
   * connecting and on a server-level error so a leftover dev/test URL is
   * visible instead of silently failing. */
  serverUrl?: string;
  /** Whether a live signaling connection exists. Create/Join are disabled
   * (with `notConnectedReason` as their title) while this is false. Default
   * true so existing callers are unaffected. */
  connected?: boolean;
  /** Title shown on the disabled Create/Join buttons while `connected` is false */
  notConnectedReason?: string;
  /** Callback when create room button is clicked */
  onCreateRoom?: () => void;
  /** Callback when join room button is clicked */
  onJoinRoom?: (code: string) => void;
  /** Callback when code input changes */
  onCodeChange?: (code: string) => void;
  /** Callback when cancel button is clicked (connecting state) */
  onCancel?: () => void;
  /** Callback when retry is clicked on a server-level error */
  onRetry?: () => void;
  /** Callback when settings button is clicked */
  onOpenSettings?: () => void;
  /** Title text (header logo) */
  title?: string;
  /** Welcome heading shown above the create/join form */
  welcomeTitle?: string;
  /** Welcome subheading shown below the welcome title */
  welcomeSubtitle?: string;
  /** Create room button text */
  createRoomText?: string;
  /** Or divider text */
  orText?: string;
  /** Code input label */
  codeLabel?: string;
  /** Code input placeholder */
  codePlaceholder?: string;
  /** Join button text */
  joinText?: string;
  /** Connecting text */
  connectingText?: string;
  /** Cancel button text */
  cancelText?: string;
  /** Invite code of the test room; the test room card shows only when given */
  testRoomCode?: string;
  /** Test room title (shown next to the code) */
  testRoomTitle?: string;
  /** Test room description */
  testRoomDescription?: string;
}

export function ConnectionPanel({
  state = "idle",
  code: controlledCode,
  errorMessage,
  errorKind = "room",
  rawErrorMessage,
  serverUrl,
  connected = true,
  notConnectedReason,
  onCreateRoom,
  onJoinRoom,
  onCodeChange,
  onCancel,
  onRetry,
  onOpenSettings,
  title = "jamjam",
  welcomeTitle: welcomeTitleProp,
  welcomeSubtitle: welcomeSubtitleProp,
  createRoomText: createRoomTextProp,
  orText: orTextProp,
  codeLabel: codeLabelProp,
  codePlaceholder: codePlaceholderProp,
  joinText: joinTextProp,
  connectingText: connectingTextProp,
  cancelText: cancelTextProp,
  testRoomCode,
  testRoomTitle: testRoomTitleProp,
  testRoomDescription: testRoomDescriptionProp,
}: ConnectionPanelProps) {
  const { t } = useTranslation();
  const welcomeTitle = welcomeTitleProp ?? t("session.welcome.title");
  const welcomeSubtitle = welcomeSubtitleProp ?? t("session.welcome.subtitle");
  const createRoomText = createRoomTextProp ?? t("session.create.button");
  const orText = orTextProp ?? t("common.label.or");
  const codeLabel = codeLabelProp ?? t("session.invite.joinByCode");
  const codePlaceholder = codePlaceholderProp ?? t("session.join.placeholder");
  const joinText = joinTextProp ?? t("session.join.button");
  const connectingText = connectingTextProp ?? t("session.join.loading");
  const cancelText = cancelTextProp ?? t("common.button.cancel");
  const testRoomTitle = testRoomTitleProp ?? t("session.testRoom.title");
  const testRoomDescription = testRoomDescriptionProp ?? t("session.testRoom.description");
  const [internalCode, setInternalCode] = useState("");
  const code = controlledCode ?? internalCode;
  const isCodeValid = isValidInviteCode(code);
  const isServerError = state === "error" && errorKind === "server";
  const hasError = state === "error" && errorKind === "room" && errorMessage;

  const handleCodeChange = useCallback(
    (value: string) => {
      const upperValue = value.toUpperCase().slice(0, INVITE_CODE_LENGTH);
      if (controlledCode === undefined) {
        setInternalCode(upperValue);
      }
      onCodeChange?.(upperValue);
    },
    [controlledCode, onCodeChange]
  );

  const handleJoin = useCallback(() => {
    if (isCodeValid && onJoinRoom) {
      onJoinRoom(code);
    }
  }, [isCodeValid, code, onJoinRoom]);

  const handleSubmit = useCallback(
    (e: FormEvent) => {
      e.preventDefault();
      handleJoin();
    },
    [handleJoin]
  );

  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (e.key === "Escape" && state === "connecting") {
        onCancel?.();
      }
    },
    [state, onCancel]
  );

  const handleTestRoomClick = useCallback(() => {
    if (onJoinRoom && testRoomCode) {
      onJoinRoom(testRoomCode);
    }
  }, [onJoinRoom, testRoomCode]);

  // Connecting state
  if (state === "connecting") {
    return (
      <div
        className="connection-panel"
        data-testid="connection-panel"
        data-state="connecting"
        onKeyDown={handleKeyDown}
      >
        <header className="connection-panel__header">
          <span className="connection-panel__logo">{title}</span>
        </header>
        <main className="connection-panel__content connection-panel__content--centered">
          <div className="connection-panel__loading" data-testid="connection-panel-loading" role="status" aria-label={connectingText}>
            <span className="connection-panel__spinner" />
            <span className="connection-panel__loading-text">{connectingText}</span>
            {serverUrl && (
              <span className="connection-panel__server-url" data-testid="connection-panel-server-url">
                {t("signaling.server.label")}: {serverUrl}
              </span>
            )}
          </div>
          {onCancel && (
            <button
              type="button"
              className="connection-panel__button connection-panel__button--secondary"
              data-testid="connection-panel-cancel"
              onClick={onCancel}
            >
              {cancelText}
            </button>
          )}
        </main>
      </div>
    );
  }

  // Idle or error state
  return (
    <div
      className="connection-panel"
      data-testid="connection-panel"
      data-state={hasError || isServerError ? "error" : "idle"}
    >
      <header className="connection-panel__header">
        <span className="connection-panel__logo">{title}</span>
        <div className="connection-panel__header-actions">
          {onOpenSettings && (
            <button
              type="button"
              className="connection-panel__icon-button"
              data-testid="connection-panel-settings"
              onClick={onOpenSettings}
              aria-label={t("settings.title")}
              title={t("settings.title")}
            >
              <SettingsIcon />
            </button>
          )}
        </div>
      </header>
      <main className="connection-panel__content">
        <form className="connection-panel__form" onSubmit={handleSubmit}>
          {/* Welcome */}
          <div className="connection-panel__welcome">
            <h1 className="connection-panel__welcome-title">{welcomeTitle}</h1>
            <p className="connection-panel__welcome-subtitle">{welcomeSubtitle}</p>
          </div>

          {/* Server-level error: cannot reach the signaling server at all. */}
          {isServerError && (
            <div className="connection-panel__server-error" data-testid="connection-panel-server-error" role="alert">
              <span className="connection-panel__server-error-icon" aria-hidden="true">
                <AlertIcon />
              </span>
              <p className="connection-panel__server-error-title">{errorMessage}</p>
              {serverUrl && (
                <p className="connection-panel__server-error-url" data-testid="connection-panel-server-error-url">
                  {t("signaling.server.label")}: {serverUrl}
                </p>
              )}
              {rawErrorMessage && (
                <p className="connection-panel__server-error-detail" data-testid="connection-panel-server-error-detail">
                  {rawErrorMessage}
                </p>
              )}
              {onRetry && (
                <button
                  type="button"
                  className="connection-panel__button connection-panel__button--secondary connection-panel__server-error-retry"
                  data-testid="connection-panel-retry"
                  onClick={onRetry}
                >
                  {t("error.action.retry")}
                </button>
              )}
            </div>
          )}

          {/* Create Room Button */}
          <button
            type="button"
            className="connection-panel__button connection-panel__button--primary"
            data-testid="connection-panel-create-room"
            onClick={onCreateRoom}
            disabled={!onCreateRoom || !connected}
            title={!connected ? notConnectedReason : undefined}
          >
            {createRoomText}
          </button>

          {/* Divider */}
          <div className="connection-panel__divider">
            <span>{orText}</span>
          </div>

          {/* Join by code */}
          <div className="connection-panel__join-section">
            <label className="connection-panel__label" htmlFor="invite-code">
              {codeLabel}
            </label>
            <div className="connection-panel__input-row">
              <input
                id="invite-code"
                data-testid="connection-panel-invite-code"
                type="text"
                className={`connection-panel__input ${hasError ? "connection-panel__input--error" : ""}`}
                value={code}
                onChange={(e) => handleCodeChange(e.target.value)}
                placeholder={codePlaceholder}
                maxLength={INVITE_CODE_LENGTH}
                autoComplete="off"
                spellCheck={false}
                aria-invalid={hasError ? "true" : undefined}
                aria-describedby={hasError ? "error-message" : undefined}
              />
              <button
                type="submit"
                className={`connection-panel__button connection-panel__button--primary connection-panel__join-button ${hasError ? "connection-panel__join-button--error" : ""}`}
                data-testid="connection-panel-join"
                disabled={!isCodeValid || !onJoinRoom || !connected}
                title={!connected ? notConnectedReason : undefined}
              >
                {joinText}
              </button>
            </div>
            {hasError && (
              <p id="error-message" className="connection-panel__error" data-testid="connection-panel-error" role="alert">
                {errorMessage}
              </p>
            )}
          </div>

          {/* Test room card */}
          {testRoomCode && onJoinRoom && (
            <button
              type="button"
              className="connection-panel__test-room"
              data-testid="connection-panel-test-room"
              onClick={handleTestRoomClick}
            >
              <span className="connection-panel__test-room-icon">
                <ZapIcon />
              </span>
              <span className="connection-panel__test-room-text">
                <span className="connection-panel__test-room-title">
                  {testRoomTitle} ({testRoomCode})
                </span>
                <span className="connection-panel__test-room-desc">{testRoomDescription}</span>
              </span>
            </button>
          )}

        </form>
      </main>
    </div>
  );
}

function AlertIcon() {
  return (
    <svg
      width="24"
      height="24"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3Z" />
      <line x1="12" x2="12" y1="9" y2="13" />
      <line x1="12" x2="12.01" y1="17" y2="17" />
    </svg>
  );
}

function SettingsIcon() {
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </svg>
  );
}

function ZapIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" stroke="none">
      <path d="M13 2 3 14h9l-1 8 10-12h-9l1-8z" />
    </svg>
  );
}

export default ConnectionPanel;
