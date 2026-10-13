import { createContext, useContext } from 'react';

/** What a page lets a transcript's message bubbles open (spec F1c §3.4):
 *  its Messages tab at a message, and another participant's transcript
 *  there. A page without a Messages tab provides nothing, and the bubbles
 *  show no links. */
export interface MessageLinks {
  openInFeed?: (id: string) => void;
  openParticipant?: (address: string, id: string) => void;
}

export const MessageLinksContext = createContext<MessageLinks>({});

export function useMessageLinks(): MessageLinks {
  return useContext(MessageLinksContext);
}
