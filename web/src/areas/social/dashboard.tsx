// Dashboard activity feed (rendered by pages/Home.tsx, which the social area owns).
import { useState } from "react";
import { Link } from "react-router-dom";
import type { User } from "../../api";
import { Blankslate } from "../../components/common";
import { sapi } from "./api";
import { EventFeed } from "./feed";

export function DashboardFeed({ user }: { user: User }) {
  const [tab, setTab] = useState<"following" | "mine">("following");
  return (
    <section className="dashboard-feed" data-testid="dashboard-feed">
      <div className="toolbar feed-head">
        <h2 className="h4">Recent activity</h2>
        <span className="spacer" />
        <div className="btn-group" role="tablist" aria-label="Feed">
          <button type="button" role="tab" aria-selected={tab === "following"} className={"btn btn-sm" + (tab === "following" ? " selected" : "")}
            onClick={() => setTab("following")} data-testid="feed-tab-following">Following</button>
          <button type="button" role="tab" aria-selected={tab === "mine"} className={"btn btn-sm" + (tab === "mine" ? " selected" : "")}
            onClick={() => setTab("mine")} data-testid="feed-tab-mine">Your activity</button>
        </div>
      </div>
      {tab === "following" ? (
        <EventFeed key="f" load={(p) => sapi.receivedEvents(user.login, p)} loadKey={`r:${user.login}`} testid="feed-following"
          empty={<Blankslate title="Nothing in your feed yet">
            Follow people and watch or star repositories to see their activity here. <Link to="/explore">Explore</Link>
          </Blankslate>} />
      ) : (
        <EventFeed key="m" load={(p) => sapi.userEvents(user.login, p)} loadKey={`u:${user.login}`} testid="feed-mine"
          empty={<Blankslate title="You don't have any recent activity" />} />
      )}
    </section>
  );
}
