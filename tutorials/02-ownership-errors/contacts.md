# Challenge 2.3: Contacts Book

Write `contacts.rk`. Build a contact book with search and removal.

## Starting Point

```rask
struct Contact {
    name: string
    email: string
    tags: Vec<string>
}

// Write functions to:
// 1. Add a contact
// 2. Search by name (partial match)
// 3. Find all contacts with a given tag
// 4. Remove a contact by name

func main() {
    let contacts = Vec.new()
    // add some contacts, search, remove, print results
}
```

## Design Questions

- When you pass a `Vec<Contact>` to a function, did you get move errors?
- How did you handle "not found" — `Option`, `Result`, or bool?
- Did you want references/borrows? Did you need `clone()`?
- How did `mutate` on the contacts Vec feel at call sites?

<details>
<summary>Hints</summary>

- Functions that modify the list need `mutate contacts: Vec<Contact>`
- Functions that search can use the default borrow (read-only)
- `name.contains(query)` for partial match
- Removing: `contacts.retain(|c| c.name != target)` or find index + remove
- Call site: `add_contact(mutate contacts, ...)`
- Search functions just borrow, so no `mutate` needed there

</details>
