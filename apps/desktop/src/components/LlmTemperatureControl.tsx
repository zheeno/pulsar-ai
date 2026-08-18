import { useId } from 'react';

type ParamSliderProps = {
  id: string;
  label: string;
  hint: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  format: (v: number) => string;
  disabled?: boolean;
  onChange: (v: number) => void;
};

function ParamSlider({
  id,
  label,
  hint,
  value,
  min,
  max,
  step = 1,
  format,
  disabled,
  onChange,
}: ParamSliderProps) {
  return (
    <div className="param-slider">
      <div className="param-slider__head">
        <div className="param-slider__copy">
          <label className="param-slider__label" htmlFor={id}>
            {label}
          </label>
          <p className="param-slider__hint">{hint}</p>
        </div>
        <span className="param-slider__value mono">{format(value)}</span>
      </div>
      <input
        id={id}
        className="param-slider__input"
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(Number(e.target.value))}
      />
      <div className="param-slider__bounds">
        <span>{format(min)}</span>
        <span>{format(max)}</span>
      </div>
    </div>
  );
}

export const LLM_TEMPERATURE_MIN = 0;
export const LLM_TEMPERATURE_MAX = 2;
export const LLM_TEMPERATURE_STEP = 0.1;
export const LLM_TEMPERATURE_DEFAULT_CUSTOM = 0.7;

export function formatLlmTemperature(value: number): string {
  return value.toFixed(1);
}

export function formatLlmTemperatureLabel(value: number | null | undefined): string {
  if (value == null) return 'Provider default';
  return formatLlmTemperature(value);
}

type LlmTemperatureControlProps = {
  useProviderDefault: boolean;
  temperature: number;
  disabled?: boolean;
  onUseProviderDefaultChange: (useDefault: boolean) => void;
  onTemperatureChange: (value: number) => void;
};

export function LlmTemperatureControl({
  useProviderDefault,
  temperature,
  disabled,
  onUseProviderDefaultChange,
  onTemperatureChange,
}: LlmTemperatureControlProps) {
  const toggleId = useId();
  const sliderId = useId();

  return (
    <div className="llm-temperature">
      <label className="toggle-row" htmlFor={toggleId}>
        <input
          id={toggleId}
          type="checkbox"
          checked={useProviderDefault}
          disabled={disabled}
          onChange={(e) => onUseProviderDefaultChange(e.target.checked)}
        />
        <span className="toggle-row__label">Use provider default</span>
      </label>
      <p className="muted llm-temperature__hint">
        Some newer models only accept the provider default. If verification fails, leave this on.
      </p>
      {!useProviderDefault ? (
        <ParamSlider
          id={sliderId}
          label="Temperature"
          hint="Lower = more deterministic. Higher = more varied."
          value={temperature}
          min={LLM_TEMPERATURE_MIN}
          max={LLM_TEMPERATURE_MAX}
          step={LLM_TEMPERATURE_STEP}
          format={formatLlmTemperature}
          disabled={disabled}
          onChange={onTemperatureChange}
        />
      ) : null}
    </div>
  );
}

export function llmTemperatureFromDraft(
  useProviderDefault: boolean,
  temperature: number,
): number | null {
  return useProviderDefault ? null : temperature;
}

export function draftFromLlmTemperature(value: number | null | undefined): {
  useProviderDefault: boolean;
  temperature: number;
} {
  if (value == null) {
    return { useProviderDefault: true, temperature: LLM_TEMPERATURE_DEFAULT_CUSTOM };
  }
  return { useProviderDefault: false, temperature: value };
}
