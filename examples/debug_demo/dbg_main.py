def add(a, b):
    return a + b


def main():
    items = [1, 2, 3, 4]
    total = add(items[0], items[1])
    acc = 0
    for i in range(5):
        acc += i
    print("total", total, acc)


if __name__ == "__main__":
    main()
