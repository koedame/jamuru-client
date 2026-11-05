/**
 * DeviceDelayField - measures the delay of the devices in use (REQ-GUI-034).
 *
 * A measuring sound is played from the output and timed to its arrival at the
 * input, so the way from one to the other has to exist: a loopback cable, or
 * the speaker placed where the microphone hears it. The figure is kept for the
 * devices it was measured on, and the others in the room are told the number.
 */

import { useTranslation } from "react-i18next";
import type { DeviceDelayStatus } from "../../../lib/tauri";
import "./TabContent.css";

export interface DeviceDelayProps {
  /** Where the measuring is; null until the app has answered */
  status: DeviceDelayStatus | null;
  /** Whether this screen may start it: the sound comes out of the speakers of the app it is connected to */
  canStart: boolean;
  /** The last try to start was refused */
  startFailed?: boolean;
  onStart: () => void;
  onCancel: () => void;
}

/** The delay to one decimal place */
function ms(value: number): string {
  return value.toFixed(1);
}

export function DeviceDelayField({ status, canStart, startFailed = false, onStart, onCancel }: DeviceDelayProps) {
  const { t } = useTranslation();
  const running = status?.running === true;
  const figure = status?.figure ?? null;
  const sessionRunning = status?.session_running === true;

  return (
    <section className="delay-meter" data-testid="device-delay">
      <h2 className="tab-content__title">{t("settings.devices.delay.title")}</h2>
      <p className="form-field__description">{t("settings.devices.delay.description")}</p>
      <ul className="delay-meter__setup">
        <li>{t("settings.devices.delay.setupCable")}</li>
        <li>{t("settings.devices.delay.setupSpeaker")}</li>
        <li>{t("settings.devices.delay.setupRoom")}</li>
      </ul>

      <div className="delay-meter__action">
        {running ? (
          <button type="button" className="delay-meter__button" onClick={onCancel} data-testid="device-delay-cancel">
            {t("settings.devices.delay.cancel")}
          </button>
        ) : (
          <button
            type="button"
            className="delay-meter__button delay-meter__button--primary"
            onClick={onStart}
            disabled={!canStart || !sessionRunning}
            data-testid="device-delay-start"
          >
            {figure ? t("settings.devices.delay.measureAgain") : t("settings.devices.delay.measure")}
          </button>
        )}
        {running && status?.stimulus && status.step != null && (
          <span className="delay-meter__progress" role="status" data-testid="device-delay-progress">
            {t("settings.devices.delay.progress", {
              sound: t(`settings.devices.delay.sound.${status.stimulus}`),
              step: status.step,
              steps: status.steps,
            })}
          </span>
        )}
      </div>

      {!canStart && <p className="form-field__description">{t("settings.devices.delay.helperCannotStart")}</p>}
      {canStart && !sessionRunning && !running && (
        <p className="form-field__description">{t("settings.devices.delay.needsRoom")}</p>
      )}
      {startFailed && !running && (
        <p className="form-field__warning" role="alert">
          {t("settings.devices.delay.startFailed")}
        </p>
      )}
      {!running && status?.failure && (
        <p className="form-field__warning" role="alert" data-testid="device-delay-failure">
          {t(`settings.devices.delay.failure.${status.failure}`)}
        </p>
      )}

      {figure ? (
        <div className="delay-meter__result" data-testid="device-delay-result">
          <span className="delay-meter__value">{t("settings.devices.delay.result", { ms: ms(figure.delay_ms) })}</span>
          <span className={`delay-meter__stability delay-meter__stability--${figure.stability}`}>
            {t(`settings.devices.delay.stability.${figure.stability}`, { spread: ms(figure.spread_ms) })}
          </span>
          {figure.needs_review && (
            <p className="form-field__warning" role="alert" data-testid="device-delay-review">
              {t("settings.devices.delay.review")}
            </p>
          )}
          <p className="form-field__description">{t("settings.devices.delay.told")}</p>
        </div>
      ) : (
        !running && <p className="form-field__description">{t("settings.devices.delay.none")}</p>
      )}
    </section>
  );
}

export default DeviceDelayField;
