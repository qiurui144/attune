/** Attune Wizard · 5 点 stepper（顶部进度指示）
 * 见 spec §3 "Stepper"
 */

import type { JSX } from 'preact';
import { t } from '../i18n';
import type { WizardStep } from './types';

export type StepperProps = {
  currentStep: WizardStep;
  completedSteps: Set<WizardStep>;
  onStepClick?: (step: WizardStep) => void;
};

const STEPS: Array<{ n: WizardStep; labelKey: string }> = [
  { n: 1, labelKey: 'wizard.step.welcome' },
  { n: 2, labelKey: 'wizard.step.password' },
  { n: 3, labelKey: 'wizard.step.llm' },
  { n: 4, labelKey: 'wizard.step.hardware' },
  { n: 5, labelKey: 'wizard.step.data' },
];

export function Stepper({
  currentStep,
  completedSteps,
  onStepClick,
}: StepperProps): JSX.Element {
  return (
    <nav
      aria-label="Setup progress"
      className="wizard-stepper-scroll"
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 'var(--space-2)',
        padding: 'var(--space-2) 0',
        width: 'max-content',
        minWidth: '100%',
      }}
    >
      {STEPS.map((step, idx) => {
        const isCompleted = completedSteps.has(step.n);
        const isCurrent = currentStep === step.n;
        const isClickable = isCompleted && onStepClick;
        const label = t(step.labelKey);

        return (
          <>
            <button
              key={step.n}
              type="button"
              onClick={isClickable ? () => onStepClick!(step.n) : undefined}
              disabled={!isClickable}
              aria-current={isCurrent ? 'step' : undefined}
              aria-label={`Step ${step.n}: ${label}${isCompleted ? ' (completed)' : ''}`}
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 'var(--space-2)',
                padding: 0,
                background: 'transparent',
                border: 'none',
                cursor: isClickable ? 'pointer' : 'default',
                whiteSpace: 'nowrap',
                flexShrink: 0,
              }}
            >
              <span
                style={{
                  display: 'inline-flex',
                  alignItems: 'center',
                  justifyContent: 'center',
                  width: 28,
                  height: 28,
                  borderRadius: '50%',
                  fontSize: 'var(--text-sm)',
                  fontWeight: 600,
                  background: isCurrent || isCompleted ? 'var(--color-accent)' : 'transparent',
                  color: isCurrent || isCompleted ? 'white' : 'var(--color-text-secondary)',
                  border:
                    isCurrent || isCompleted
                      ? '2px solid var(--color-accent)'
                      : '2px solid var(--color-border)',
                  boxShadow: isCurrent
                    ? '0 0 0 4px rgba(94, 139, 139, 0.2)'
                    : undefined,
                  transition: 'all var(--duration-base) var(--ease-out)',
                }}
              >
                {isCompleted && !isCurrent ? '✓' : step.n}
              </span>
              <span
                className="wizard-step-label"
                style={{
                  fontSize: 'clamp(12px, 1.3vw, var(--text-sm))',
                  fontWeight: isCurrent ? 600 : 400,
                  color: isCurrent
                    ? 'var(--color-text)'
                    : isCompleted
                      ? 'var(--color-text)'
                      : 'var(--color-text-secondary)',
                }}
              >
                {label}
              </span>
            </button>
            {idx < STEPS.length - 1 && (
              <span
                aria-hidden="true"
                style={{
                  width: 'clamp(12px, 2vw, 32px)',
                  flexShrink: 0,
                  height: 2,
                  background: completedSteps.has(step.n)
                    ? 'var(--color-accent)'
                    : 'var(--color-border)',
                  borderRadius: 1,
                  transition: 'background var(--duration-base) var(--ease-out)',
                }}
              />
            )}
          </>
        );
      })}
    </nav>
  );
}
