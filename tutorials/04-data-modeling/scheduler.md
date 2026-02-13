# Challenge 4.2: Task Scheduler

Write `scheduler.rk`. Build a priority-based task scheduler.

## Starting Point

```rask
enum Priority { Low, Medium, High, Critical }

struct Task {
    id: i32
    name: string
    priority: Priority
    completed: bool
}

// Build a scheduler that:
// 1. Adds tasks with priorities
// 2. Gets next task (highest priority first)
// 3. Marks tasks complete
// 4. Lists pending tasks

func main() {
    // Create tasks, schedule them, process highest priority first
}
```

## Design Questions

- Did you want `Ord` / `PartialOrd` on `Priority`? How did you compare?
- Was `Vec<Task>` enough or did you want a priority queue?
- How much code for the sorting/priority logic?
- Did `extend Priority` for comparison feel natural?

<details>
<summary>Hints</summary>

- Add a `rank` method to Priority via `extend Priority { func rank(self) -> i32 { ... } }`
- Use `Vec<Task>` as the backing store — no need for a fancy priority queue
- For "get next": sort by priority rank descending, find first non-completed
- Or: `tasks.filter(|t| !t.completed).max_by_key(|t| t.priority.rank())`
- Mark complete by ID: find the task and set `completed = true`
- List pending: `tasks.filter(|t| !t.completed)`

</details>
