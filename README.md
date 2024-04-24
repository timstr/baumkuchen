# baumkuchen

A simple and minimalist static HTML site generator. Use it when all you really need is to copy and paste some HTML around.

## Basic Usage

1. Start with ordinary HTML pages, either written from scratch or taken from an existing HTML website.
2. Identify repeated structure and extract it into a separate html file, defining a new element
3. Use the new element in your HTML pages as a shorthand
4. Run baumkuchen to substitute and expand elements, producing a complete HTML site.

## Example

Suppose we have a page like this:

```html
<html>
    <body>
        <div class="icon-grid">
            <div class="icon-row">
                <div class="icon-outer">
                    <img src="a.png" />
                </div>
                <div class="icon-outer">
                    <img src="b.png" />
                </div>
                <div class="icon-outer">
                    <img src="c.png" />
                </div>
            </div>
            <div class="icon-row">
                <div class="icon-outer">
                    <img src="d.png" />
                </div>
                <div class="icon-outer">
                    <img src="e.png" />
                </div>
                <div class="icon-outer">
                    <img src="f.png" />
                </div>
            </div>
        </div>
    </body>
</html>
```

There's already a ton of repetition in this basic example. Let's first carve out the `<div class="icon-outer">` and its contents. To do that, let's create a file `elements/myicon.html` with the following:

```html
<div class="icon-outer">
    <img src="${self.src}" />
</div>
```

This defines a new element called `myicon` which takes a `src` attribute. This lets us simplify the above page into:

```html
<html>
    <body>
        <div class="icon-grid">
            <div class="icon-row">
                <myicon src="a.png" />
                <myicon src="b.png" />
                <myicon src="c.png" />
            </div>
            <div class="icon-row">
                <myicon src="d.png" />
                <myicon src="e.png" />
                <myicon src="f.png" />
            </div>
        </div>
    </body>
</html>
```

We could keep going if we like. For example, given `elements/myicongrid.html`:

```html
<div class="icon-grid">
    <self.inner />
</div>
```

and `elements/myiconrow.html`:

```html
<div class="icon-row">
    <foreachchild.x>
        <x />
    </foreachchild.x>
</div>
```

The page now can be reduced to

```html
<html>
    <body>
        <myicongrid>
            <myiconrow>
                <myicon src="a.png" />
                <myicon src="b.png" />
                <myicon src="c.png" />
            </myiconrow>
            <myiconrow>
                <myicon src="d.png" />
                <myicon src="e.png" />
                <myicon src="f.png" />
            </myiconrow>
        </myicongrid>
    </body>
</html>
```

We run baumkuchen by passing it the path to the shorthand HTML pages, elements, and output directory, respectively.

```plaintext
> baumkuchen path/to/pages/ elements/ output/
```

Afterwards, `output/` here contains all files (not just html) copied from the input directory, with any HTML files expanded according to the provided element library.

A few other utilities exist currently such as `<if>` elements:

```html
<if self.filepath="/artwork/.*">
    <then>
        <artworktabmenu />
    </then>
</if>
```

and maybe a couple others as I create them.

## Caveats

-   This library is new and experimental
-   This is all the documentation that exists so far
-   Yes, this is an abuse of notation.
-   Efforts have been made to keep the syntax friendly for HTML text editors, but yours may not like the non-standard tag names
-   Baumkuchen is not aware of what tags mean in any way. It just substitutes and expands the ones with names matching elmement files.

## About the name

[Baumkuchen](https://en.wikipedia.org/wiki/Baumkuchen) is German (like me) for a type of layer cake. It literally means "tree cake". Here, "tree" is a nod to the DOM tree, and "cake" as in baking, as in we're baking a static website from a higher-level authoring format.
