import { useEffect, useState } from "react";
import { FORGE, request } from "../api";

interface AnnouncementData {
  announcement: string | null;
  expires_at: string | null;
  user_dismissible: boolean;
  html: string | null;
}

const DISMISS_KEY = "forge-announcement-dismissed";

function dismissedText(): string | null {
  try {
    return localStorage.getItem(DISMISS_KEY);
  } catch {
    return null;
  }
}

/** The enterprise announcement banner (set by site admins via PATCH /enterprise/announcement). */
export function AnnouncementBanner() {
  const [data, setData] = useState<AnnouncementData | null>(null);
  const [dismissed, setDismissed] = useState<string | null>(dismissedText);
  useEffect(() => {
    let live = true;
    request<AnnouncementData>(`${FORGE}/announcement`)
      .then((d) => {
        if (live) setData(d);
      })
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, []);
  if (!data?.announcement || !data.html) return null;
  if (data.user_dismissible && dismissed === data.announcement) return null;
  return (
    <div className="announcement-banner" role="region" aria-label="Announcement" data-testid="announcement-banner">
      <div className="markdown-body announcement-body" dangerouslySetInnerHTML={{ __html: data.html }} />
      {data.user_dismissible && (
        <button type="button" className="announcement-dismiss" aria-label="Dismiss announcement"
          onClick={() => {
            try {
              localStorage.setItem(DISMISS_KEY, data.announcement || "");
            } catch {
              /* storage unavailable */
            }
            setDismissed(data.announcement);
          }}>
          ×
        </button>
      )}
    </div>
  );
}
