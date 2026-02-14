# Challenge 1.3: Word Counter

Write `wordcount.rk`. Given a hardcoded multi-line string:
1. Count total words
2. Count unique words (use a `Map`)
3. Print the top 5 most frequent words

## Starting Point

```rask
func main() {
    const text = "the quick brown fox jumps over the lazy dog
the fox was quick and the dog was lazy
over and over the fox jumped"

    // 1. Split into words, count total
    // 2. Build a frequency map
    // 3. Find the top 5
}
```

## Design Questions

- How does `string.split_whitespace()` → Vec feel?
- Is iterating a `Map` natural?
- Did you want an `.entry()` API or is `map.get()` + `map.insert()` fine?
- How did you sort to find the top 5?

<details>
<summary>Hints</summary>

- `text.split_whitespace()` gives you a Vec of words
- `Map.new()` creates an empty map
- To increment a counter: check `map.get(word)`, then `map.insert(word, count + 1)`
- Or use `map.ensure_modify(word, || 1, |count| { count += 1 })` for upsert
- To sort by frequency, you may need to collect entries into a Vec and sort

</details>
