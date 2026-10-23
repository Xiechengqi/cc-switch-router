import { c2cDescriptionParts } from "@/lib/market-c2c";

/** Seller-authored plain text. No HTML, embeds, link previews or custom schemes. */
export function C2CDescription({ value, links = true }: { value: string; links?: boolean }) {
  return <p data-c2c-description className="min-w-0 whitespace-pre-wrap break-words text-sm leading-6 [overflow-wrap:anywhere]">
    {links ? c2cDescriptionParts(value).map((part, index) => part.href
      ? <a key={index} href={part.href} target="_blank" rel="noopener noreferrer nofollow ugc" className="text-primary underline underline-offset-2 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">{part.text}</a>
      : part.text) : value}
  </p>;
}
