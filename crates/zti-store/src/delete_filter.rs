const DELETE_FILTER_BATCH_SIZE: usize = 1;

pub(crate) fn file_path_delete_filters(paths: &[&str]) -> Vec<String> {
    let mut filters = Vec::with_capacity(paths.len().div_ceil(DELETE_FILTER_BATCH_SIZE));
    paths
        .chunks(DELETE_FILTER_BATCH_SIZE)
        .map(file_path_delete_filter)
        .for_each(|filter| filters.push(filter));
    filters
}

fn file_path_delete_filter(paths: &[&str]) -> String {
    let estimated_len = paths
        .iter()
        .map(|path| "file_path = ''".len() + path.len() + " OR ".len())
        .sum();
    let mut filter = String::with_capacity(estimated_len);
    paths.iter().enumerate().for_each(|(index, path)| {
        if index > 0 {
            filter.push_str(" OR ");
        }
        filter.push_str("file_path = '");
        push_escaped_sql_string(path, &mut filter);
        filter.push('\'');
    });
    filter
}

fn push_escaped_sql_string(value: &str, out: &mut String) {
    value.chars().for_each(|ch| {
        if ch == '\'' {
            out.push('\'');
        }
        out.push(ch);
    });
}
