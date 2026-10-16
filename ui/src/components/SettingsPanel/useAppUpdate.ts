/**
 * useAppUpdate - the About tab's update section against the backend (ADR-082)
 *
 * Reads the running version and what this build can do about a newer release,
 * checks when asked (by the button, or by the menu through `checkRequest`) and
 * installs when asked. An install ends in a restart, so a successful one never
 * comes back to this code.
 */

import { useCallback, useEffect, useState } from "react";
import {
  updateCheck,
  updateInstall,
  updateStatus,
  windowIsInSession,
  type UpdateSupport,
} from "../../lib/tauri";
import { useWindowEvent } from "../../hooks/useWindowEvents";
import type { UpdatePhase, UpdateSectionProps } from "./tabs/UpdateSection";

/**
 * @param checkRequest Checks each time it changes to a number above 0
 */
export function useAppUpdate(checkRequest = 0): UpdateSectionProps {
  const [current, setCurrent] = useState<string | null>(null);
  const [support, setSupport] = useState<UpdateSupport | null>(null);
  const [phase, setPhase] = useState<UpdatePhase>("idle");
  const [version, setVersion] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [inSession, setInSession] = useState(false);

  useEffect(() => {
    updateStatus()
      .then((status) => {
        setCurrent(status.current);
        setSupport(status.support);
      })
      .catch((err) => {
        console.error("Could not read the update status:", err);
        setError(String(err));
        setPhase("error");
      });
    windowIsInSession()
      .then(setInSession)
      .catch(() => {});
  }, []);
  const enterSession = useCallback(() => setInSession(true), []);
  const leaveSession = useCallback(() => setInSession(false), []);
  useWindowEvent("session:connected", enterSession);
  useWindowEvent("session:disconnected", leaveSession);

  const check = useCallback(async () => {
    setPhase("checking");
    setError(null);
    try {
      const checked = await updateCheck();
      if (checked.kind === "unsupported") {
        setSupport("none");
        setPhase("idle");
      } else if (checked.kind === "available") {
        setVersion(checked.version);
        setPhase("available");
      } else {
        setPhase("up_to_date");
      }
    } catch (err) {
      console.error("Could not check for a newer release:", err);
      setError(String(err));
      setPhase("error");
    }
  }, []);

  useEffect(() => {
    if (checkRequest > 0) void check();
  }, [checkRequest, check]);

  const install = useCallback(async () => {
    setPhase("installing");
    setError(null);
    try {
      const installed = await updateInstall();
      switch (installed.kind) {
        case "in_session":
          setInSession(true);
          setPhase("available");
          break;
        case "busy":
          setPhase("busy");
          break;
        case "up_to_date":
          setPhase("up_to_date");
          break;
        case "unsupported":
          setSupport("none");
          setPhase("idle");
          break;
        case "restarting":
          setVersion(installed.version);
          break;
      }
    } catch (err) {
      console.error("Could not install the newer release:", err);
      setError(String(err));
      setPhase("error");
    }
  }, []);

  return {
    current,
    support,
    phase,
    version,
    error,
    inSession,
    onCheck: check,
    onInstall: install,
  };
}
