import DOMPurify from "dompurify";

const ALLOWED_TAGS = [
  "p",
  "br",
  "strong",
  "em",
  "u",
  "a",
  "ul",
  "ol",
  "li",
  "h1",
  "h2",
  "h3",
  "h4",
  "blockquote",
  "code",
  "pre",
  "span",
  "div",
];

export function sanitizeAnnouncementHtml(html: string): string {
  const sanitized = DOMPurify.sanitize(html, {
    ALLOWED_TAGS,
    ALLOWED_ATTR: ["href", "target", "rel", "class"],
  });
  if (typeof window === "undefined") return sanitized;
  const template = document.createElement("template");
  template.innerHTML = sanitized;
  template.content.querySelectorAll("a[target='_blank']").forEach((anchor) => {
    const rel = anchor.getAttribute("rel") || "";
    const tokens = new Set(rel.split(/\s+/).filter(Boolean));
    tokens.add("noopener");
    tokens.add("noreferrer");
    anchor.setAttribute("rel", Array.from(tokens).join(" "));
  });
  const wrapper = document.createElement("div");
  wrapper.appendChild(template.content);
  return wrapper.innerHTML;
}
