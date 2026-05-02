fn main() {
    let path = std::env::args().nth(1).expect("usage: chunker_diag <path>");
    let content = std::fs::read_to_string(&path).expect("read");
    let sections = attune_core::chunker::extract_sections_with_path(&content);
    println!("Total sections: {}", sections.len());
    for (i, s) in sections.iter().enumerate().take(20) {
        let preview: String = s.content.chars().take(40).collect();
        println!("  [{}] depth={} path={:?} len={} '{}'", i, s.path.len(), s.path.last(), s.content.len(), preview);
    }
    if sections.len() > 20 {
        println!("  ... and {} more", sections.len() - 20);
    }

    let mut total_chunks = 0;
    for (i, s) in sections.iter().enumerate() {
        let chunks = attune_core::chunker::chunk(&s.content, attune_core::chunker::DEFAULT_CHUNK_SIZE, attune_core::chunker::DEFAULT_OVERLAP);
        let bytes = s.content.len();
        let chars = s.content.chars().count();
        println!("  Section[{}] bytes={} chars={} → {} chunks (chunk_size=512)", i, bytes, chars, chunks.len());
        total_chunks += chunks.len();
    }
    println!("Total L2 chunks (sum chunker::chunk per section): {}", total_chunks);
    println!("Total L1+L2 = {} + {} = {}", sections.len(), total_chunks, sections.len() + total_chunks);
}
