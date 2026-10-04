import type { CompileResult, Diagnostic, Severity } from './compiler';
import type { ExchangeOutcome, Step } from './consumer';
import type { ProviderEvent } from './provider';
import type { ChainReport } from './types';

// What the playground says about what happened, as data. These are pure, so the wording is tested without a browser; ui.ts
// only draws them.

export const STEPS: Step[] = ['discover', 'quote', 'deal', 'result', 'verify'];

/** Who signs each record, as the page names them. */
export const SIGNER: Record<string, string> = { descriptor: 'Bob', offer: 'Bob', quote: 'Bob', deal: 'Alice', receipt: 'Bob' };

export const short = (text: string, length = 12) => (text.length > length ? `${text.slice(0, length)}…` : text);

export function sizeLabel(bytes: number): string {
  if (bytes < 1024) return `${bytes} bytes`;
  return `${(bytes / 1024).toFixed(bytes < 10_240 ? 1 : 0)} KB`;
}

export interface OutcomeView {
  status: 'verified' | 'failed' | 'refused';
  title: string;
  detail: string;
  hint?: string;
  result?: string;
}

/** What the page says about one exchange. */
export function describeOutcome(outcome: ExchangeOutcome): OutcomeView {
  if (outcome.ok && outcome.status === 'succeeded') {
    return {
      status: 'verified',
      title: 'Verified',
      detail: 'Alice checked all five signatures and the links between them, and the receipt commits to the result she received.',
      result: JSON.stringify(outcome.result, null, 2),
    };
  }
  if (outcome.ok) {
    return {
      status: 'failed',
      title: 'Verified, but the function failed',
      detail: `Bob's signed receipt says ${outcome.failure!.code}: ${outcome.failure!.message}`,
      hint: 'A failure is a signed outcome too. This evidence verifies, and it shows what Bob did.',
    };
  }
  const hint = outcome.reason.includes('result_hash')
    ? "The signature on Bob's receipt is genuine, so the records still verify. Alice caught it because she hashes the result she received and compares it with the hash Bob signed."
    : outcome.reason.includes('does not verify: its signature')
      ? 'Bob checks the signature on every deal before he runs anything. A change after signing breaks it, so he ran nothing.'
      : outcome.stage === 'input'
        ? 'Nothing was sent.'
        : undefined;
  return {
    status: 'refused',
    title: outcome.stage === 'input' ? 'Not sent' : `${outcome.refusedBy === 'alice' ? 'Alice' : 'Bob'} refused`,
    detail: outcome.reason,
    ...(hint ? { hint } : {}),
  };
}

export function describeChain(report: ChainReport): { valid: boolean; text: string } {
  if (report.valid) return { valid: true, text: 'All five signatures verify, and each record names the one before it by hash.' };
  const broken = report.artifacts.filter((artifact) => artifact.status === 'invalid').map((artifact) => artifact.artifact_type);
  if (broken.length) return { valid: false, text: `Verification failed: the ${broken.join(' and ')} no longer matches what was signed.` };
  return { valid: false, text: report.error ?? 'Verification failed: the records do not form a complete chain.' };
}

export function describeEvent(event: Extract<ProviderEvent, { type: 'executed' }>): string {
  const ms = Math.round(event.ms);
  if (event.outcome.ok) return `Bob ran ${event.serviceId} in ${ms} ms.`;
  if (event.outcome.timedOut) return `Bob stopped ${event.serviceId} at his time limit (${ms} ms).`;
  return `${event.serviceId} failed after ${ms} ms: ${event.outcome.error}`;
}

export interface ProblemItem {
  severity: Severity;
  message: string;
  /** Where it is in the editor, counted from 1, when it is in the person's code. */
  line: number | null;
  column: number | null;
  endLine: number | null;
  endColumn: number | null;
  note?: string;
}

export interface ProblemsView {
  status: 'ok' | 'warning' | 'error';
  summary: string;
  items: ProblemItem[];
}

/** Said of a problem the compiler finds in the contract after the code. */
export const CONTRACT_NOTE = 'The compiler reports this in the contract that follows your code. Common causes: a bracket left open, or a function named alloc or run.';

const item = (diagnostic: Diagnostic): ProblemItem => ({
  severity: diagnostic.severity,
  message: diagnostic.message,
  line: diagnostic.line,
  column: diagnostic.column,
  endLine: diagnostic.endLine,
  endColumn: diagnostic.endColumn,
  ...(diagnostic.inContract ? { note: CONTRACT_NOTE } : {}),
});

/** What the page says about one compile: how it went, and each problem with its place. */
export function describeCompile(result: CompileResult): ProblemsView {
  if (result.ok) {
    return {
      status: result.warnings.length ? 'warning' : 'ok',
      summary: `Compiled ${sizeLabel(result.module.length)} in ${result.ms} ms. It imports nothing, as a Froglet function must.`,
      items: result.warnings.map(item),
    };
  }
  if (result.kind === 'errors') {
    const count = result.diagnostics.filter((diagnostic) => diagnostic.severity === 'error').length;
    return { status: 'error', summary: `${count} problem${count === 1 ? '' : 's'} in the code.`, items: result.diagnostics.map(item) };
  }
  return { status: 'error', summary: result.message, items: [] };
}
