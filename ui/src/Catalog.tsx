/**
 * UI Component Catalog
 *
 * Development tool for previewing and testing UI components.
 * Run with: npm run dev:catalog
 */
import { useState, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { ConnectionIndicator, type ConnectionStatus } from "./components/ConnectionIndicator";

function Catalog() {
  const { t, i18n } = useTranslation();
  const [status, setStatus] = useState<ConnectionStatus>("disconnected");
  const [latency, setLatency] = useState<number | undefined>(undefined);

  // Update html lang attribute when language changes
  useEffect(() => {
    document.documentElement.lang = i18n.language;
  }, [i18n.language]);

  const statuses: ConnectionStatus[] = [
    "disconnected",
    "connecting",
    "connected",
    "unstable",
    "error",
  ];

  const handleStatusChange = (newStatus: ConnectionStatus) => {
    setStatus(newStatus);
    // Set latency for connected/unstable states
    if (newStatus === "connected") {
      setLatency(15);
    } else if (newStatus === "unstable") {
      setLatency(120);
    } else {
      setLatency(undefined);
    }
  };

  return (
    <main
      style={{
        padding: "var(--space-lg)",
        fontFamily: "var(--font-family-sans)",
        backgroundColor: "var(--color-bg-primary)",
        color: "var(--color-text-primary)",
        minHeight: "100vh",
      }}
    >
      <header style={{ marginBottom: "var(--space-lg)" }}>
        <h1 style={{ fontSize: "var(--font-size-h1)", marginBottom: "var(--space-xs)" }}>
          UI Component Catalog
        </h1>
        <p style={{ color: "var(--color-text-secondary)" }}>
          jamuru - Development Preview
        </p>
      </header>

      {/* Language switcher */}
      <section style={{ marginBottom: "var(--space-xl)" }}>
        <h2 style={{ fontSize: "var(--font-size-h2)", marginBottom: "var(--space-sm)" }}>
          {t("settings.display.language")}
        </h2>
        <div style={{ display: "flex", gap: "var(--space-sm)" }}>
          <button
            onClick={() => i18n.changeLanguage("ja")}
            style={{
              padding: "var(--padding-button)",
              borderRadius: "var(--radius-md)",
              border: "none",
              backgroundColor:
                i18n.language === "ja" ? "var(--color-accent)" : "var(--color-bg-secondary)",
              color:
                i18n.language === "ja" ? "var(--color-text-inverse)" : "var(--color-text-primary)",
              cursor: "pointer",
            }}
          >
            日本語
          </button>
          <button
            onClick={() => i18n.changeLanguage("en")}
            style={{
              padding: "var(--padding-button)",
              borderRadius: "var(--radius-md)",
              border: "none",
              backgroundColor:
                i18n.language === "en" ? "var(--color-accent)" : "var(--color-bg-secondary)",
              color:
                i18n.language === "en" ? "var(--color-text-inverse)" : "var(--color-text-primary)",
              cursor: "pointer",
            }}
          >
            English
          </button>
        </div>
      </section>

      {/* ConnectionIndicator demo */}
      <section style={{ marginBottom: "var(--space-xl)" }}>
        <h2 style={{ fontSize: "var(--font-size-h2)", marginBottom: "var(--space-sm)" }}>
          ConnectionIndicator
        </h2>

        {/* Current status display */}
        <div
          style={{
            padding: "var(--padding-card)",
            backgroundColor: "var(--color-bg-secondary)",
            borderRadius: "var(--radius-lg)",
            marginBottom: "var(--space-md)",
          }}
        >
          <ConnectionIndicator status={status} latencyMs={latency} size="lg" />
        </div>

        {/* Status selector */}
        <div style={{ display: "flex", gap: "var(--space-sm)", flexWrap: "wrap" }}>
          {statuses.map((s) => (
            <button
              key={s}
              onClick={() => handleStatusChange(s)}
              style={{
                padding: "var(--padding-button)",
                borderRadius: "var(--radius-md)",
                border: "none",
                backgroundColor:
                  status === s ? "var(--color-accent)" : "var(--color-bg-secondary)",
                color:
                  status === s ? "var(--color-text-inverse)" : "var(--color-text-primary)",
                cursor: "pointer",
              }}
            >
              {s}
            </button>
          ))}
        </div>
      </section>

      {/* Size variants */}
      <section style={{ marginBottom: "var(--space-xl)" }}>
        <h2 style={{ fontSize: "var(--font-size-h2)", marginBottom: "var(--space-sm)" }}>
          Size Variants
        </h2>
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            gap: "var(--space-md)",
            padding: "var(--padding-card)",
            backgroundColor: "var(--color-bg-secondary)",
            borderRadius: "var(--radius-lg)",
          }}
        >
          <div>
            <span style={{ color: "var(--color-text-secondary)", marginRight: "var(--space-sm)" }}>
              sm:
            </span>
            <ConnectionIndicator status="connected" latencyMs={15} size="sm" />
          </div>
          <div>
            <span style={{ color: "var(--color-text-secondary)", marginRight: "var(--space-sm)" }}>
              md:
            </span>
            <ConnectionIndicator status="connected" latencyMs={15} size="md" />
          </div>
          <div>
            <span style={{ color: "var(--color-text-secondary)", marginRight: "var(--space-sm)" }}>
              lg:
            </span>
            <ConnectionIndicator status="connected" latencyMs={15} size="lg" />
          </div>
        </div>
      </section>

      {/* All states */}
      <section>
        <h2 style={{ fontSize: "var(--font-size-h2)", marginBottom: "var(--space-sm)" }}>
          All States
        </h2>
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            gap: "var(--space-sm)",
            padding: "var(--padding-card)",
            backgroundColor: "var(--color-bg-secondary)",
            borderRadius: "var(--radius-lg)",
          }}
        >
          <ConnectionIndicator status="disconnected" />
          <ConnectionIndicator status="connecting" />
          <ConnectionIndicator status="connected" latencyMs={15} />
          <ConnectionIndicator status="unstable" latencyMs={120} />
          <ConnectionIndicator status="error" />
        </div>
      </section>
    </main>
  );
}

export default Catalog;
