for file in $(ls test-ymls/*.yml); do
    cargo run -q $file > $file.out 2>&1
done
