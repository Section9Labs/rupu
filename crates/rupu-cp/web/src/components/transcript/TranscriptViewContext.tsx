/**
 * TranscriptViewContext — the transcript reader's shared view state: which
 * event-kind filter is active and the expand-all / collapse-all signal.
 *
 * `expandTarget` says what the last expand-all/collapse-all asked for and
 * `expandNonce` increments on every such request, so a consumer reacts to the
 * nonce changing (even when the target repeats). Without a provider the
 * default is inert: filter `all`, no expand request.
 */

import { createContext, useContext } from 'react';
import type { ReactNode } from 'react';
import type { FilterKind } from './eventKind';

export interface TranscriptViewState {
  filter: FilterKind;
  expandTarget: 'open' | 'closed' | null;
  expandNonce: number;
  /** The run is still being tailed: a call with no result yet is genuinely running. */
  live: boolean;
}

const DEFAULT_STATE: TranscriptViewState = {
  filter: 'all',
  expandTarget: null,
  expandNonce: 0,
  live: false,
};

const TranscriptViewContext = createContext<TranscriptViewState>(DEFAULT_STATE);

export function TranscriptViewProvider({
  value,
  children,
}: {
  value: TranscriptViewState;
  children: ReactNode;
}) {
  return <TranscriptViewContext.Provider value={value}>{children}</TranscriptViewContext.Provider>;
}

export function useTranscriptView(): TranscriptViewState {
  return useContext(TranscriptViewContext);
}
