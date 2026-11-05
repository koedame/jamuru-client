/**
 * The device delay measurement as the Devices tab shows it (REQ-GUI-034): where
 * the run is, the figure for the devices selected now, and the start and stop.
 *
 * The status is read from the app the screen is connected to - the helper's
 * window reads the helped person's. Starting is left to the person whose
 * devices they are: the sound comes out of their speakers.
 */
import { useCallback, useEffect, useState } from "react";
import { useWindowEvent } from "../../hooks/useWindowEvents";
import {
  AUDIO_SETTINGS_CHANGED,
  DEVICE_DELAY_MEASURED,
  delayMeasureCancel,
  delayMeasureStart,
  delayMeasureStatus,
  type DeviceDelayStatus,
} from "../../lib/tauri";
import type { DeviceDelayProps } from "./tabs/DeviceDelayField";

const POLL_RUNNING_MS = 400;
const POLL_IDLE_MS = 2000;

/** `canStart` is false where the sound would come out of someone else's speakers. */
export function useDeviceDelay(canStart: boolean): DeviceDelayProps {
  const [status, setStatus] = useState<DeviceDelayStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  const read = useCallback(() => {
    delayMeasureStatus()
      .then(setStatus)
      .catch((e) => console.error("Could not read the device delay:", e));
  }, []);

  const running = status?.running === true;
  useEffect(() => {
    read();
    const timer = setInterval(read, running ? POLL_RUNNING_MS : POLL_IDLE_MS);
    return () => clearInterval(timer);
  }, [read, running]);
  // The devices changed under the figure, or a run ended: read at once
  useWindowEvent<unknown>(AUDIO_SETTINGS_CHANGED, read);
  useWindowEvent<unknown>(DEVICE_DELAY_MEASURED, read);

  const start = useCallback(() => {
    setError(null);
    delayMeasureStart()
      .then(read)
      .catch((e) => {
        console.error("Could not start measuring the device delay:", e);
        setError(typeof e === "string" ? e : "failed");
        read();
      });
  }, [read]);
  const cancel = useCallback(() => {
    delayMeasureCancel()
      .then(read)
      .catch((e) => console.error("Could not stop measuring the device delay:", e));
  }, [read]);

  return {
    status,
    canStart,
    startFailed: error !== null,
    onStart: start,
    onCancel: cancel,
  };
}
