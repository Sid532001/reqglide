import { describe, expect, it } from "vitest";
import { composeRequestUrl, splitRequestUrl } from "./request-url";

describe("request URL query synchronization", () => {
  it("splits duplicate and encoded parameters from a URL", () => {
    expect(splitRequestUrl("https://example.com/users?tag=a&tag=b&name=Ada+Lovelace#top"))
      .toEqual({
        url: "https://example.com/users#top",
        queryParams: [
          { name: "tag", value: "a", enabled: true },
          { name: "tag", value: "b", enabled: true },
          { name: "name", value: "Ada Lovelace", enabled: true },
        ],
      });
  });

  it("composes enabled parameters and preserves variable tokens", () => {
    expect(
      composeRequestUrl("https://example.com/users#top", [
        { name: "owner", value: "{{userId}}", enabled: true },
        { name: "disabled", value: "value", enabled: false },
      ]),
    ).toBe("https://example.com/users?owner={{userId}}#top");
  });
});
