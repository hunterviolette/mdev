import type { Page } from 'playwright';

export type InteractionProfile = {
  responseSelector: string;
  uploadInputSelector?: string;
  uploadButtonSelector?: string;
  chatInputSelector?: string;
  chatSubmitSelector?: string;
};

const profiles: Record<string, InteractionProfile> = {
  auto: {
    responseSelector: '[data-message-author-role="assistant"], [data-testid="message-content"], message-content, .model-response-text, .response-container, .assistant, .assistant-message, [data-role="assistant"]',
    uploadInputSelector: 'input[type="file"]',
    uploadButtonSelector: 'button[aria-label*="upload" i], button[title*="upload" i], button[type="button"]',
    chatInputSelector: '#prompt-textarea, textarea, div[contenteditable="true"], [role="textbox"]',
    chatSubmitSelector: 'button[data-testid="send-button"], button[type="submit"], [aria-label*="send" i]'
  }
};

const autoCandidates: InteractionProfile[] = [
  {
    responseSelector: '.relative.group.items-start .message-bubble',
    uploadInputSelector: 'input[type="file"]',
    uploadButtonSelector: 'button[aria-label*="upload" i], button[title*="upload" i], button[type="button"]',
    chatInputSelector: '#prompt-textarea, textarea, div[contenteditable="true"], [role="textbox"]',
    chatSubmitSelector: 'button[data-testid="send-button"], button[type="submit"], [aria-label*="send" i]'
  }
];

export function getInteractionProfile(name?: string): InteractionProfile {
  const key = (name ?? 'auto').trim().toLowerCase();
  return profiles[key] ?? profiles.auto;
}

async function countVisible(page: Page, selector?: string): Promise<number> {
  if (!selector) {
    return 0;
  }

  try {
    const locator = page.locator(selector);
    const count = await locator.count();
    let visible = 0;

    for (let i = 0; i < Math.min(count, 5); i++) {
      if (await locator.nth(i).isVisible().catch(() => false)) {
        visible++;
      }
    }

    return visible;
  } catch {
    return 0;
  }
}

async function scoreInteractionProfile(page: Page, profile: InteractionProfile): Promise<number> {
  const [chatInput, chatSubmit, response, uploadInput, uploadButton] = await Promise.all([
    countVisible(page, profile.chatInputSelector),
    countVisible(page, profile.chatSubmitSelector),
    countVisible(page, profile.responseSelector),
    countVisible(page, profile.uploadInputSelector),
    countVisible(page, profile.uploadButtonSelector)
  ]);

  return chatInput * 5 + chatSubmit * 3 + response * 2 + uploadInput + uploadButton;
}

export async function resolveInteractionProfile(page: Page, requestedName?: string): Promise<{ name: string; profile: InteractionProfile }> {
  const requested = (requestedName ?? 'auto').trim().toLowerCase();

  if (requested !== 'auto') {
    return {
      name: requested in profiles ? requested : 'auto',
      profile: profiles[requested] ?? profiles.auto
    };
  }

  let bestProfile = profiles.auto;
  let bestScore = await scoreInteractionProfile(page, profiles.auto);

  for (const candidate of autoCandidates) {
    const score = await scoreInteractionProfile(page, candidate);
    if (score > bestScore) {
      bestScore = score;
      bestProfile = candidate;
    }
  }

  return {
    name: 'auto',
    profile: bestProfile
  };
}
