# Server-rendered page metadata

A document's `Title`, `Meta`, and `Link` can use top-level page bindings.
The server evaluates those expressions when it renders the full document and
escapes the resulting values. No browser script is required.

```asx
page Story() {
  data story = loadPublishedStory(params.slug)
  const title = story.title + " | My Journal"

  return ASX {
    <Head>
      <Title>{title}</Title>
      <Meta name="description" content={story.description} />
      <Link rel="canonical" href={story.canonical} />
    </Head>
    <article>{story.title}</article>
  }
}
```

A null `Meta` content or `Link` href omits the corresponding element.
Set the canonical URL from a trusted application setting; this renderer
escapes HTML but does not validate URLs or decide which content is public.

Layout assets are preserved and a page title overrides the layout title.
In composed layouts, loader/constant bindings used by the head must remain
in the top-level scope (for example, a top-level layout `<Slot />`). Bindings
inside a component, loop, or conditional body are not document metadata.
Fragment rendering does not evaluate the head, and this change does not make
script URLs or theme preflight settings dynamic.
