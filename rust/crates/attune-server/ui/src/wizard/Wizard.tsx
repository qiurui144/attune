/** Attune Wizard · 5 步首次配置向导
 * 见 spec §3 "Wizard 5 步详细流程"
 */

import type { JSX } from 'preact';
import { useState, useCallback } from 'preact/hooks';
import { Button } from '../components';
import { t } from '../i18n';
import { Stepper } from './Stepper';
import { Step1Welcome } from './Step1Welcome';
import { Step2Password } from './Step2Password';
import { Step3LLM } from './Step3LLM';
import { Step4Hardware } from './Step4Hardware';
import { Step5Data } from './Step5Data';
import { WizardDone } from './WizardDone';
import { initialWizardContext } from './types';
import type { WizardContext, WizardStep } from './types';

export type WizardProps = {
  onComplete: () => void;
  onSkipAll?: () => void;
};

export function Wizard({ onComplete, onSkipAll }: WizardProps): JSX.Element {
  const [ctx, setCtx] = useState<WizardContext>(initialWizardContext);
  const [doneShown, setDoneShown] = useState(false);
  const panelMaxWidth = ctx.step === 3 ? 1160 : 900;

  const goNext = useCallback(() => {
    setCtx((prev) => {
      const completed = new Set(prev.completedSteps);
      completed.add(prev.step);
      if (prev.step === 5) {
        return { ...prev, completedSteps: completed };
      }
      return {
        ...prev,
        step: (prev.step + 1) as WizardStep,
        completedSteps: completed,
      };
    });
  }, []);

  const goBack = useCallback(() => {
    setCtx((prev) =>
      prev.step > 1
        ? { ...prev, step: (prev.step - 1) as WizardStep }
        : prev,
    );
  }, []);

  const jumpTo = useCallback((step: WizardStep) => {
    setCtx((prev) => ({ ...prev, step }));
  }, []);

  const updateCtx = useCallback((partial: Partial<WizardContext>) => {
    setCtx((prev) => ({ ...prev, ...partial }));
  }, []);

  const finish = useCallback(() => {
    setCtx((prev) => {
      const completed = new Set(prev.completedSteps);
      completed.add(5);
      return { ...prev, completedSteps: completed };
    });
    setDoneShown(true);
  }, []);

  // 完成页短显示后触发 onComplete
  if (doneShown) {
    return <WizardDone onContinue={onComplete} />;
  }

  return (
    <div
      style={{
        minHeight: '100vh',
        background:
          'radial-gradient(ellipse at top right, #E9EEF2 0%, #F7F8FA 50%)',
        display: 'flex',
        flexDirection: 'column',
      }}
    >
      {/* 顶栏：品牌 + stepper + 跳过全部 */}
      <header
        style={{
          padding: 'clamp(var(--space-3), 2.5vw, var(--space-4)) clamp(var(--space-3), 4vw, var(--space-6))',
          display: 'flex',
          alignItems: 'center',
          gap: 'var(--space-4)',
          flexWrap: 'wrap',
          borderBottom: '1px solid transparent',
        }}
      >
        <span
          style={{
            fontWeight: 600,
            fontSize: 'var(--text-base)',
            color: 'var(--color-text)',
            flexShrink: 0,
          }}
        >
          🌿 <span className="wizard-brand-text">{t('app.name')}</span>
        </span>
        <div
          className="wizard-stepper-scroll"
          style={{
            flex: 1,
            display: 'flex',
            justifyContent: 'center',
            minWidth: 0,
            overflowX: 'auto',
            overflowY: 'hidden',
          }}
        >
          <Stepper
            currentStep={ctx.step}
            completedSteps={ctx.completedSteps}
            onStepClick={jumpTo}
          />
        </div>
        {onSkipAll && (
          <button
            type="button"
            onClick={onSkipAll}
            style={{
              background: 'transparent',
              border: 'none',
              color: 'var(--color-text-secondary)',
              fontSize: 'var(--text-sm)',
              cursor: 'pointer',
              padding: 'var(--space-1) var(--space-2)',
            }}
          >
            {t('wizard.skip_all')}
          </button>
        )}
      </header>

      {/* 主区：居中白色卡片 */}
      <section
        style={{
          flex: 1,
          display: 'flex',
          alignItems: 'flex-start',
          justifyContent: 'center',
          padding: 'clamp(var(--space-3), 3vw, var(--space-5))',
        }}
      >
        <div
          className="fade-slide-in"
          key={ctx.step}
          style={{
            background: 'var(--color-surface)',
            borderRadius: 'var(--radius-xl)',
            boxShadow: 'var(--shadow-lg)',
            padding: 'clamp(var(--space-4), 3vw, var(--space-7)) clamp(var(--space-4), 3vw, var(--space-6))',
            width: `min(96vw, ${panelMaxWidth}px)`,
            minHeight: 'min(70vh, 560px)',
            display: 'flex',
            flexDirection: 'column',
          }}
        >
          <div style={{ flex: 1 }}>
            {ctx.step === 1 && (
              <Step1Welcome onContinue={goNext} onImport={onSkipAll ?? onComplete} />
            )}
            {ctx.step === 2 && (
              <Step2Password
                ctx={ctx}
                onUpdate={updateCtx}
                onContinue={goNext}
              />
            )}
            {ctx.step === 3 && (
              <Step3LLM
                ctx={ctx}
                onUpdate={updateCtx}
                onContinue={goNext}
              />
            )}
            {ctx.step === 4 && (
              <Step4Hardware
                ctx={ctx}
                onUpdate={updateCtx}
                onContinue={goNext}
              />
            )}
            {ctx.step === 5 && (
              <Step5Data
                ctx={ctx}
                onUpdate={updateCtx}
                onFinish={finish}
              />
            )}
          </div>

          {/* 底部导航（Welcome 和 Done 自管理，其他步骤统一） */}
          {ctx.step >= 2 && ctx.step <= 4 && (
            <footer
              style={{
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'space-between',
                marginTop: 'var(--space-6)',
                paddingTop: 'var(--space-5)',
                borderTop: '1px solid var(--color-border)',
              }}
            >
              <Button variant="ghost" onClick={goBack}>
                ← {t('common.back')}
              </Button>
              {/* 下一步按钮由各 step 内部渲染（因为要控制 disabled 状态） */}
              <div id="wizard-step-primary-action" />
            </footer>
          )}
        </div>
      </section>
    </div>
  );
}
